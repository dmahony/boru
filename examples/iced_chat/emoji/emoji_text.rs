//! Mixed text + Twemoji message rendering (BORU-TWEMOJI-17).
//!
//! [`emoji_text`] renders a chat message body as normal Boru text with
//! inline Twemoji SVGs, using the shared grapheme parser
//! (BORU-TWEMOJI-16), the shared resolver (BORU-TWEMOJI-07) and the
//! shared renderer/SVG cache (BORU-TWEMOJI-08/09).
//!
//! Guardrails:
//! - **Presentation only.** The input `&str` is never modified. The
//!   original full Unicode string remains the message content — copy,
//!   selection, accessibility and message actions all keep operating on
//!   the plain Unicode message, never on asset keys or SVG paths.
//! - **Fallback, never suppression.** Unknown/newer emoji stay inside
//!   their text run (parser behaviour), and an emoji whose SVG handle
//!   cannot be produced falls back to its original Unicode text
//!   (BORU-TWEMOJI-20). Nothing is ever replaced by an empty widget.
//! - **Order and gaps.** Fragments render in input order with zero
//!   inter-fragment spacing, so mixed text/emoji keeps its order and
//!   multiple adjacent emoji have no unwanted gaps or forced line breaks.
//!
//! Line wrapping and baseline alignment *polish* are BORU-TWEMOJI-18;
//! this component deliberately keeps the fragment-row approach simple
//! (mirroring the URL-segment row pattern already used in chat bubbles).

use iced::widget::svg;
use iced::widget::Row;
use iced::{Alignment, Color, ContentFit, Element, Font, Length};

use super::parser::{split_fragments, MessageFragment};
use super::renderer::EmojiRenderer;

/// Emoji artwork size as a multiple of the surrounding text size.
///
/// Twemoji SVGs use a 36x36 viewBox whose glyph fills most of the square,
/// so a ~1.25x scale makes the emoji read at roughly the cap height of the
/// surrounding text. Exact baseline/line-height alignment is
/// BORU-TWEMOJI-18; this scale keeps the artwork visually in family until
/// then.
pub const EMOJI_TEXT_SCALE: f32 = 1.25;

/// Typography applied to the text runs of an [`emoji_text`] render.
///
/// Mirrors the exact `text()` styling the chat bubble already applies to a
/// plain message body (size, font, line height, wrapping, color) so emoji
/// messages inherit Boru's existing message typography verbatim.
#[derive(Debug, Clone, Copy)]
pub struct EmojiTextStyle {
    /// Text size in pixels (Boru's `chat_text_size`).
    pub size: f32,
    /// Message font (Boru's `TypeRole::ChatMessage` font).
    pub font: Font,
    /// Line height (Boru's `TypeRole::ChatMessage` line height).
    pub line_height: iced::widget::text::LineHeight,
    /// Word/glyph wrapping mode (Boru uses `WordOrGlyph`).
    pub wrapping: iced::widget::text::Wrapping,
    /// Text color (local/remote/system body color from the theme).
    pub color: Color,
}

/// One ordered piece of an emoji-aware message body, after splitting and
/// artwork resolution (testable without building Iced widgets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmojiTextArtwork<'a> {
    /// A plain text run rendered with the message typography.
    Text(&'a str),
    /// A resolved emoji grapheme rendered as a Twemoji SVG.
    Svg {
        /// The original Unicode grapheme (what stays in the message).
        unicode: &'a str,
        /// Normalized Twemoji asset key (e.g. `"1f600"`), for tests and
        /// diagnostics. Presentation metadata only — never message content.
        key: &'static str,
        /// Cached SVG handle for the vendored asset.
        handle: svg::Handle,
    },
}

/// Resolve a message string into ordered artwork pieces using the shared
/// parser and renderer.
///
/// Order matches input. Adjacent text pieces (plain runs plus emoji that
/// fall back to Unicode) are coalesced into one [`Text`] run so a message
/// whose emoji cannot be rendered degrades to exactly the pre-Twemoji
/// plain-text element — and [`emoji_text`] takes its fast path. An emoji
/// grapheme whose SVG handle cannot be produced (missing/unreadable
/// vendored file) degrades to its original Unicode text — never to an
/// empty widget and never to a broken image (BORU-TWEMOJI-20).
///
/// [`Text`]: EmojiTextArtwork::Text
pub fn plan_emoji_text<'a>(
    renderer: &impl EmojiRenderer,
    input: &'a str,
) -> Vec<EmojiTextArtwork<'a>> {
    let mut plan: Vec<EmojiTextArtwork<'a>> = Vec::new();
    for fragment in split_fragments(input) {
        match fragment {
            MessageFragment::Text(text) => {
                extend_text_run(&mut plan, input, text);
            }
            MessageFragment::Emoji { unicode, asset } => {
                match renderer.svg_handle(&asset) {
                    Some(handle) => plan.push(EmojiTextArtwork::Svg {
                        unicode,
                        key: asset.key,
                        handle,
                    }),
                    // Fallback: render the original Unicode grapheme as
                    // part of the surrounding text run.
                    None => extend_text_run(&mut plan, input, unicode),
                }
            }
        }
    }
    plan
}

/// Append `piece` to the plan, merging it into the trailing [`Text`] run
/// when one is open. `input` is the original message string; `piece` must
/// be a (contiguous) sub-slice of `input`, which `split_fragments` and the
/// grapheme fallback guarantee. Merging keeps fallback text in one run —
/// and lets `emoji_text` return a single plain `text()` element when the
/// whole message degrades.
///
/// [`Text`]: EmojiTextArtwork::Text
fn extend_text_run<'a>(plan: &mut Vec<EmojiTextArtwork<'a>>, input: &'a str, piece: &'a str) {
    if let Some(EmojiTextArtwork::Text(prev)) = plan.last_mut() {
        // Byte-range merge over `input` (both slices live inside it).
        let base = input.as_ptr() as usize;
        let start = prev.as_ptr() as usize - base;
        let end = piece.as_ptr() as usize + piece.len() - base;
        *prev = &input[start..end];
    } else {
        plan.push(EmojiTextArtwork::Text(piece));
    }
}

/// Build a text widget with the message typography from [`EmojiTextStyle`].
fn styled_text<'a, Message: 'a + Clone>(
    content: &'a str,
    style: &EmojiTextStyle,
) -> Element<'a, Message> {
    iced::widget::text(content)
        .size(style.size)
        .font(style.font)
        .line_height(style.line_height)
        .wrapping(style.wrapping)
        .color(style.color)
        .into()
}

/// Render a message body as normal Boru text plus inline Twemoji SVGs.
///
/// - Plain text runs use the caller's message typography (via
///   [`EmojiTextStyle`]), exactly like the bubble's existing `text()`
///   element.
/// - Resolved emoji render as square Twemoji SVGs sized relative to the
///   text size ([`EMOJI_TEXT_SCALE`]).
/// - The input string is borrowed, never copied or rewritten; the message
///   data (and therefore copy/selection/message actions) remains the
///   original full Unicode string.
///
/// The returned element is a wrapping row of text/SVG children with zero
/// spacing, mirroring the URL-segment row already used in chat bubbles.
pub fn emoji_text<'a, Message: 'a + Clone>(
    renderer: &impl EmojiRenderer,
    input: &'a str,
    style: &EmojiTextStyle,
) -> Element<'a, Message> {
    let plan = plan_emoji_text(renderer, input);

    // Fast path: no emoji at all — render exactly like the bubble's plain
    // `text()` element today (no row wrapper, identical typography).
    if let [EmojiTextArtwork::Text(content)] = plan.as_slice() {
        return styled_text(content, style);
    }

    let emoji_size = style.size * EMOJI_TEXT_SCALE;
    let mut row = Row::new().spacing(0).align_y(Alignment::Center);
    for item in plan {
        match item {
            EmojiTextArtwork::Text(content) => {
                row = row.push(styled_text(content, style));
            }
            EmojiTextArtwork::Svg { handle, .. } => {
                row = row.push(
                    svg(handle)
                        .width(Length::Fixed(emoji_size))
                        .height(Length::Fixed(emoji_size))
                        .content_fit(ContentFit::Contain),
                );
            }
        }
    }
    row.wrap().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stub renderer that resolves like the real one but can never
    /// produce an SVG handle — exercises the fallback path deterministically
    /// without reading vendored files.
    struct NoHandleRenderer;

    impl EmojiRenderer for NoHandleRenderer {
        fn resolve(&self, grapheme: &str) -> Option<super::super::renderer::EmojiAsset> {
            super::super::parser::emoji_asset(grapheme)
        }

        fn svg_handle(&self, _asset: &super::super::renderer::EmojiAsset) -> Option<svg::Handle> {
            None
        }
    }

    fn style() -> EmojiTextStyle {
        EmojiTextStyle {
            size: 15.0,
            font: Font::DEFAULT,
            line_height: iced::widget::text::LineHeight::Relative(1.45),
            wrapping: iced::widget::text::Wrapping::WordOrGlyph,
            color: Color::BLACK,
        }
    }

    #[test]
    fn plan_plain_text_is_single_text_artwork() {
        let r = super::super::renderer::TwemojiRenderer;
        let plan = plan_emoji_text(&r, "hello world");
        assert_eq!(plan, vec![EmojiTextArtwork::Text("hello world")]);
    }

    #[test]
    fn plan_mixed_text_and_emoji_preserve_order() {
        let r = super::super::renderer::TwemojiRenderer;
        let plan = plan_emoji_text(&r, "hi 😀 there 🍕 bye");
        assert_eq!(plan.len(), 5);
        assert_eq!(plan[0], EmojiTextArtwork::Text("hi "));
        match &plan[1] {
            EmojiTextArtwork::Svg { unicode, key, .. } => {
                assert_eq!(*unicode, "😀");
                assert_eq!(*key, "1f600");
            }
            other => panic!("expected Svg artwork, got {other:?}"),
        }
        assert_eq!(plan[2], EmojiTextArtwork::Text(" there "));
        match &plan[3] {
            EmojiTextArtwork::Svg { unicode, key, .. } => {
                assert_eq!(*unicode, "🍕");
                assert_eq!(*key, "1f355");
            }
            other => panic!("expected Svg artwork, got {other:?}"),
        }
        assert_eq!(plan[4], EmojiTextArtwork::Text(" bye"));
    }

    #[test]
    fn plan_adjacent_emoji_have_no_gap_artwork() {
        let r = super::super::renderer::TwemojiRenderer;
        // Two emoji back-to-back: two Svg pieces, no empty Text between.
        let plan = plan_emoji_text(&r, "😀🍕");
        assert_eq!(plan.len(), 2);
        for item in &plan {
            assert!(
                matches!(item, EmojiTextArtwork::Svg { .. }),
                "expected only Svg artwork, got {item:?}"
            );
        }
    }

    #[test]
    fn plan_roundtrips_original_unicode_string() {
        let r = super::super::renderer::TwemojiRenderer;
        let input = "hello 😀 world 🇺🇸 👍🏻 \u{1f469}\u{200d}\u{1f4bb} bye";
        let plan = plan_emoji_text(&r, input);
        let joined: String = plan
            .iter()
            .map(|item| match item {
                EmojiTextArtwork::Text(t) => *t,
                EmojiTextArtwork::Svg { unicode, .. } => *unicode,
            })
            .collect();
        assert_eq!(joined, input);
    }

    #[test]
    fn plan_unknown_emoji_stays_in_text_run() {
        let r = super::super::renderer::TwemojiRenderer;
        // 🫩 (Unicode 16.0, not vendored) stays inside the text run as
        // original Unicode — never suppressed, never an empty widget.
        let plan = plan_emoji_text(&r, "hi 🫩 bye");
        assert_eq!(plan, vec![EmojiTextArtwork::Text("hi 🫩 bye")]);
    }

    #[test]
    fn plan_falls_back_to_unicode_when_svg_handle_missing() {
        let r = NoHandleRenderer;
        // 😀 resolves to a vendored asset, but the stub renderer cannot
        // produce a handle: the artwork degrades to the original Unicode
        // grapheme, not a broken/empty image.
        let plan = plan_emoji_text(&r, "hi 😀 bye");
        assert_eq!(plan, vec![EmojiTextArtwork::Text("hi 😀 bye")]);
    }

    #[test]
    fn emoji_scale_is_sane_relative_to_text() {
        // 1.25x of a 15px message keeps the artwork in family with the
        // text; exact alignment polish is BORU-TWEMOJI-18.
        assert!((EMOJI_TEXT_SCALE - 1.25).abs() < f32::EPSILON);
        assert!(15.0 * EMOJI_TEXT_SCALE > 15.0);
    }
}
