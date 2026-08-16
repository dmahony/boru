//! Mixed text + Twemoji message rendering (BORU-TWEMOJI-17, hardened for
//! wrapping + baseline in BORU-TWEMOJI-18).
//!
//! [`emoji_text`] renders a chat message body as normal Boru text with
//! inline Twemoji SVGs, using the shared grapheme parser
//! (BORU-TWEMOJI-16), the shared resolver (BORU-TWEMOJI-07) and the
//! shared renderer/SVG cache (BORU-TWEMOJI-08/09).
//!
//! # Inline layout (BORU-TWEMOJI-18)
//!
//! The message is rendered as ONE cosmic-text paragraph with spans — the
//! same layout engine the plain `text()` widget uses — so wrapping,
//! line height and baseline behave exactly like ordinary chat text:
//!
//! - Text runs become text spans carrying Boru's message typography.
//! - Each resolved emoji becomes an *invisible placeholder span* whose
//!   advance width equals the emoji box size (`EMOJI_TEXT_SCALE` × text
//!   size) but whose font metrics stay at 1.0× — measured empirically so
//!   emoji never inflate a line's height (no blank lines) and never shift
//!   the line baseline (no vertical jitter).
//! - After layout, [`Paragraph::span_bounds`] reports exactly where each
//!   placeholder landed (after wrapping), and the Twemoji SVG is drawn
//!   into that rectangle, centered on the line box.
//!
//! The placeholder is `EM SPACE + WORD JOINER + FOUR-PER-EM SPACE`
//! (`\u{2003}\u{2060}\u{2005}`): 1em + 0em + 0.25em = 1.25em of advance at
//! 1.0× metrics. The word joiner keeps the two spaces unbreakable so the
//! emoji box never splits across lines.
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
//! - **Order and gaps.** Spans are built in input order with zero extra
//!   spacing, so mixed text/emoji keeps its order and multiple adjacent
//!   emoji have no unwanted gaps or forced line breaks.

use std::marker::PhantomData;

use iced::advanced::layout;
use iced::advanced::renderer;
use iced::advanced::text::{
    self, Alignment, Difference, LineHeight, Paragraph as _, Shaping, Span, Text as TextDef,
    Wrapping,
};
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::widget::Operation;
use iced::advanced::widget::Widget;
use iced::advanced::{Clipboard, Layout, Shell};
use iced::widget::svg;
use iced::{Color, Element, Font, Length, Pixels, Point, Rectangle, Size, Theme};

// Brings `draw_svg` (svg::Renderer) into scope for the custom widget.
use iced::advanced::svg::Renderer as _SvgRenderer;

use super::parser::{split_fragments, MessageFragment};
use super::renderer::EmojiRenderer;

/// Emoji artwork size as a multiple of the surrounding text size.
///
/// Twemoji SVGs use a 36x36 viewBox whose glyph fills most of the square,
/// so a ~1.25x scale makes the emoji read at roughly the cap height of the
/// surrounding text. This is also the *advance width* reserved by the
/// invisible placeholder span inside the message paragraph, so the emoji
/// box occupies exactly `EMOJI_TEXT_SCALE` × text size of horizontal space
/// on the text baseline without inflating the line box.
pub const EMOJI_TEXT_SCALE: f32 = 1.25;

/// Invisible placeholder text for one emoji inside the message paragraph.
///
/// Measured with the app's message font (Figtree 15px):
/// - EM SPACE (`\u{2003}`)      = 1.00em advance
/// - WORD JOINER (`\u{2060}`)   = 0.00em advance (prevents a line break
///   between the two space glyphs, so the box stays atomic)
/// - FOUR-PER-EM SPACE (`\u{2005}`) = 0.25em advance
///
/// Total: exactly `EMOJI_TEXT_SCALE` × text size. Because the span keeps
/// the message font size (1.0×), cosmic-text computes line height and
/// baseline identically to a plain text run — an emoji never stretches a
/// line (no blank lines) and never shifts the baseline (no jitter).
/// A transparent color hides the space glyphs; only the reserved advance
/// remains for the SVG drawn over [`Paragraph::span_bounds`].
const EMOJI_PLACEHOLDER: &str = "\u{2003}\u{2060}\u{2005}";

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
    pub line_height: LineHeight,
    /// Word/glyph wrapping mode (Boru uses `WordOrGlyph`).
    pub wrapping: Wrapping,
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
/// The fallback decision is the shared [`EmojiRenderer::artwork`] rule:
/// SVG when the grapheme resolves to a vendored asset whose SVG loads,
/// original Unicode text otherwise — identical to the picker
/// (BORU-TWEMOJI-10) and any future emoji surface. This function already
/// holds the resolved asset from [`split_fragments`], so it calls
/// [`EmojiRenderer::svg_handle`] directly instead of re-resolving through
/// `artwork`; the rule it applies is the same.
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
/// - Emoji-free messages take the fast path: a single plain `text()`
///   element with the message typography, byte-for-byte the pre-Twemoji
///   render.
/// - Mixed messages render through [`EmojiText`], a custom widget that
///   lays out the whole message as one span-based paragraph (cosmic-text
///   handles wrapping/baseline/line-height exactly like plain chat text)
///   and draws each Twemoji SVG into the placeholder box where the emoji
///   landed after wrapping.
///
/// The input string is borrowed, never copied or rewritten; the message
/// data (and therefore copy/selection/message actions) remains the
/// original full Unicode string.
pub fn emoji_text<'a, Message: 'a + Clone>(
    renderer: &impl EmojiRenderer,
    input: &'a str,
    style: &EmojiTextStyle,
) -> Element<'a, Message> {
    let plan = plan_emoji_text(renderer, input);

    // Fast path: no emoji at all — render exactly like the bubble's plain
    // `text()` element today (no widget wrapper, identical typography).
    if let [EmojiTextArtwork::Text(content)] = plan.as_slice() {
        return styled_text(content, style);
    }

    EmojiText {
        plan,
        style: *style,
        input,
        _message: PhantomData,
    }
    .into()
}

/// Cached paragraph state for [`EmojiText`], mirroring iced's own `Rich`
/// text widget: the built cosmic-text paragraph plus a fingerprint of the
/// content it was built from, so scrolling a long conversation reuses the
/// shaped paragraph instead of re-shaping on every frame.
#[derive(Default)]
struct State {
    paragraph: <iced::Renderer as iced::advanced::text::Renderer>::Paragraph,
    /// Concatenated span text the paragraph was built from (text runs plus
    /// placeholder glyphs). Content change → full re-shape.
    content_key: Option<String>,
}

/// A custom widget that renders one chat message body as inline text +
/// Twemoji SVG, wrapping and baselines handled by a single cosmic-text
/// paragraph (BORU-TWEMOJI-18).
pub struct EmojiText<'a, Message> {
    plan: Vec<EmojiTextArtwork<'a>>,
    style: EmojiTextStyle,
    input: &'a str,
    _message: PhantomData<Message>,
}

impl<'a, Message> EmojiText<'a, Message> {
    /// Build the span list for the whole message: text runs with the
    /// message typography, emoji as invisible fixed-advance placeholders.
    fn spans(&self) -> Vec<Span<'a, (), Font>> {
        self.plan
            .iter()
            .map(|item| match item {
                EmojiTextArtwork::Text(content) => Span::new(*content)
                    .size(self.style.size)
                    .font(self.style.font)
                    .color(self.style.color),
                EmojiTextArtwork::Svg { .. } => Span::new(EMOJI_PLACEHOLDER)
                    .size(self.style.size)
                    .font(self.style.font)
                    .color(Color::TRANSPARENT),
            })
            .collect()
    }

    /// The text content of the spans, as a fingerprint for the paragraph
    /// cache. Text runs contribute their Unicode; emoji contribute a
    /// marker so any change in which emoji are present re-shapes.
    fn content_key(&self) -> String {
        let mut key = String::with_capacity(self.input.len() + self.plan.len());
        for item in &self.plan {
            match item {
                EmojiTextArtwork::Text(content) => key.push_str(content),
                EmojiTextArtwork::Svg { unicode, .. } => {
                    // Marker + the grapheme itself: a different emoji at
                    // the same position is a different content.
                    key.push('\u{fffc}');
                    key.push_str(unicode);
                }
            }
        }
        key
    }

    /// Build (or reuse) the paragraph for the current spans, returning its
    /// minimum bounds as the widget's layout node.
    fn paragraph_node(
        &self,
        state: &mut State,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::sized(limits, Length::Shrink, Length::Shrink, |limits| {
            let bounds = limits.max();
            let spans = self.spans();

            #[derive(Clone, Copy)]
            struct Format {
                bounds: Size,
                size: Pixels,
                line_height: LineHeight,
                font: Font,
                wrapping: Wrapping,
            }
            let format = Format {
                bounds,
                size: Pixels(self.style.size),
                line_height: self.style.line_height,
                font: self.style.font,
                wrapping: self.style.wrapping,
            };

            let build_paragraph = || {
                <iced::Renderer as iced::advanced::text::Renderer>::Paragraph::with_spans(TextDef {
                    content: spans.as_slice(),
                    bounds: format.bounds,
                    size: format.size,
                    line_height: format.line_height,
                    font: format.font,
                    align_x: Alignment::Left,
                    align_y: iced::alignment::Vertical::Top,
                    shaping: Shaping::Advanced,
                    wrapping: format.wrapping,
                })
            };

            let key = self.content_key();
            match &state.content_key {
                Some(prev) if *prev == key => {
                    // Same content: only re-shape when the format changed.
                    match state.paragraph.compare(TextDef {
                        content: (),
                        bounds: format.bounds,
                        size: format.size,
                        line_height: format.line_height,
                        font: format.font,
                        align_x: Alignment::Left,
                        align_y: iced::alignment::Vertical::Top,
                        shaping: Shaping::Advanced,
                        wrapping: format.wrapping,
                    }) {
                        Difference::None => {}
                        Difference::Bounds => {
                            state.paragraph.resize(bounds);
                        }
                        Difference::Shape => {
                            state.paragraph = build_paragraph();
                        }
                    }
                }
                _ => {
                    state.paragraph = build_paragraph();
                    state.content_key = Some(key);
                }
            }

            state.paragraph.min_bounds()
        })
    }
}

impl<'a, Message> From<EmojiText<'a, Message>> for Element<'a, Message, Theme, iced::Renderer>
where
    Message: 'a + Clone,
{
    fn from(widget: EmojiText<'a, Message>) -> Self {
        Element::new(widget)
    }
}

impl<'a, Message> Widget<Message, Theme, iced::Renderer> for EmojiText<'a, Message>
where
    Message: 'a + Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Shrink,
            height: Length::Shrink,
        }
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_mut::<State>();
        self.paragraph_node(state, renderer, limits)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        defaults: &renderer::Style,
        layout: Layout<'_>,
        _cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        if !layout.bounds().intersects(viewport) {
            return;
        }

        let state = tree.state.downcast_ref::<State>();

        // 1. Draw the whole paragraph (text runs + invisible emoji
        //    placeholders) with the message typography.
        iced::advanced::widget::text::draw(
            renderer,
            defaults,
            layout.bounds(),
            &state.paragraph,
            iced::advanced::widget::text::Style {
                color: Some(self.style.color),
            },
            viewport,
        );

        // 2. Draw each Twemoji SVG into the rectangle its placeholder
        //    span occupied after wrapping.
        let translation = layout.position() - Point::ORIGIN;
        let emoji_size = self.style.size * EMOJI_TEXT_SCALE;

        for (span_index, item) in self.plan.iter().enumerate() {
            let EmojiTextArtwork::Svg { handle, .. } = item else {
                continue;
            };

            for rect in state.paragraph.span_bounds(span_index) {
                let rect = rect + translation;
                // The span rect is (advance ≈ emoji_size) × (line height).
                // Fit the square SVG inside it, centered on the line box —
                // the emoji then sits on the text baseline like a glyph.
                let size = emoji_size.min(rect.width).min(rect.height);
                let bounds = Rectangle::new(
                    Point::new(rect.center_x() - size / 2.0, rect.center_y() - size / 2.0),
                    Size::new(size, size),
                );
                renderer.draw_svg(
                    iced::advanced::svg::Svg::new(handle.clone()),
                    bounds,
                    *viewport,
                );
            }
        }
    }

    fn operate(
        &mut self,
        _tree: &mut Tree,
        layout: Layout<'_>,
        _renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        // Keep the original full Unicode string reachable for copy,
        // selection and accessibility (guardrail: presentation only).
        operation.text(None, layout.bounds(), self.input);
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        _layout: Layout<'_>,
        _cursor: iced::advanced::mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        iced::advanced::mouse::Interaction::None
    }
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
            line_height: LineHeight::Relative(1.45),
            wrapping: Wrapping::WordOrGlyph,
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

    /// BORU-TWEMOJI-20 acceptance: fallback does NOT alter the stored or
    /// copied message text. When every emoji in a message must fall back
    /// (missing/unreadable SVG), the plan still reproduces the input
    /// byte-for-byte — the original Unicode, including multi-codepoint
    /// graphemes, is never replaced by an asset key, path, or empty run.
    #[test]
    fn fallback_plan_roundtrips_original_text_when_all_emoji_missing() {
        let r = NoHandleRenderer;
        let input = "hello 😀 world 🇺🇸 👍🏻 \u{1f469}\u{200d}\u{1f4bb} 🫩 bye";
        let plan = plan_emoji_text(&r, input);
        let joined: String = plan
            .iter()
            .map(|item| match item {
                EmojiTextArtwork::Text(t) => *t,
                EmojiTextArtwork::Svg { unicode, .. } => *unicode,
            })
            .collect();
        assert_eq!(joined, input, "fallback must preserve original text");
        // Every piece is a text run — nothing renders as a broken image.
        assert!(
            plan.iter()
                .all(|item| matches!(item, EmojiTextArtwork::Text(_))),
            "all-emissing renderer must produce only text runs: {plan:?}"
        );
    }

    #[test]
    fn emoji_scale_is_sane_relative_to_text() {
        // 1.25x of a 15px message keeps the artwork in family with the
        // text; the placeholder reserves exactly this advance.
        assert!((EMOJI_TEXT_SCALE - 1.25).abs() < f32::EPSILON);
        assert!(15.0 * EMOJI_TEXT_SCALE > 15.0);
    }

    #[test]
    fn placeholder_advance_reserves_exactly_one_em_quarter_more() {
        // The placeholder is EM SPACE (1em) + WORD JOINER (0em) +
        // FOUR-PER-EM SPACE (0.25em): at 1.0x metrics this is exactly
        // 1.25 × the text size — the same as EMOJI_TEXT_SCALE. Measured
        // with the paragraph engine to catch font metric regressions.
        let spans = vec![Span::<(), Font>::new(EMOJI_PLACEHOLDER)
            .size(Pixels(15.0))
            .font(Font::DEFAULT)];
        let para =
            <iced::Renderer as iced::advanced::text::Renderer>::Paragraph::with_spans(TextDef {
                content: &spans,
                bounds: Size::new(400.0, f32::INFINITY),
                size: Pixels(15.0),
                line_height: LineHeight::Relative(1.45),
                font: Font::DEFAULT,
                align_x: Alignment::Left,
                align_y: iced::alignment::Vertical::Top,
                shaping: Shaping::Advanced,
                wrapping: Wrapping::WordOrGlyph,
            });
        let bounds = para.span_bounds(0);
        assert_eq!(bounds.len(), 1, "placeholder must stay on one line");
        let width = bounds[0].width;
        assert!(
            (width - 15.0 * EMOJI_TEXT_SCALE).abs() < 0.5,
            "placeholder advance {width} should be ~{} (1.25 × 15)",
            15.0 * EMOJI_TEXT_SCALE
        );
        // And critically: line height must be the same as a plain text run
        // (1.45 × 15 = 21.75), so an emoji never inflates the line.
        assert!(
            (para.min_bounds().height - 21.75).abs() < 0.5,
            "placeholder line height {} should be ~21.75 (no blank lines)",
            para.min_bounds().height
        );
    }

    #[test]
    fn placeholder_line_metrics_match_plain_text() {
        // A message with an emoji placeholder must produce the same line
        // height AND the same baseline positions as the same text without
        // the placeholder — otherwise emoji lines would sit higher/lower
        // than their neighbours (vertical jitter).
        let text_para = || {
            let spans = vec![Span::<(), Font>::new("hello world wraps here ok")];
            <iced::Renderer as iced::advanced::text::Renderer>::Paragraph::with_spans(TextDef {
                content: &spans,
                bounds: Size::new(80.0, f32::INFINITY),
                size: Pixels(15.0),
                line_height: LineHeight::Relative(1.45),
                font: Font::DEFAULT,
                align_x: Alignment::Left,
                align_y: iced::alignment::Vertical::Top,
                shaping: Shaping::Advanced,
                wrapping: Wrapping::WordOrGlyph,
            })
        };
        let emoji_para = || {
            let spans = vec![
                Span::<(), Font>::new("hello world wraps here "),
                Span::<(), Font>::new(EMOJI_PLACEHOLDER).color(Color::TRANSPARENT),
                Span::<(), Font>::new(" ok"),
            ];
            <iced::Renderer as iced::advanced::text::Renderer>::Paragraph::with_spans(TextDef {
                content: &spans,
                bounds: Size::new(80.0, f32::INFINITY),
                size: Pixels(15.0),
                line_height: LineHeight::Relative(1.45),
                font: Font::DEFAULT,
                align_x: Alignment::Left,
                align_y: iced::alignment::Vertical::Top,
                shaping: Shaping::Advanced,
                wrapping: Wrapping::WordOrGlyph,
            })
        };

        let plain = text_para();
        let emoji = emoji_para();

        assert_eq!(
            plain.min_bounds().height,
            emoji.min_bounds().height,
            "emoji placeholder must not inflate total height"
        );

        // Compare baseline positions line by line: line_y of every layout
        // run must match (same line_top + same centering offsets).
        let plain_runs: Vec<_> = plain.buffer().layout_runs().map(|r| r.line_y).collect();
        let emoji_runs: Vec<_> = emoji.buffer().layout_runs().map(|r| r.line_y).collect();
        assert_eq!(
            plain_runs.len(),
            emoji_runs.len(),
            "wrapping must produce the same number of lines"
        );
        for (i, (p, e)) in plain_runs.iter().zip(emoji_runs.iter()).enumerate() {
            assert!(
                (p - e).abs() < 0.01,
                "baseline on line {i} differs: plain {p} vs emoji {e}"
            );
        }
    }

    /// Build a span paragraph for `content` wrapped at `width`, returning
    /// the paragraph plus per-span bounds (for emoji placeholders).
    fn paragraph_with_spans(spans: Vec<Span<'_, (), Font>>, width: f32) -> Paragraph {
        <iced::Renderer as iced::advanced::text::Renderer>::Paragraph::with_spans(TextDef {
            content: &spans,
            bounds: Size::new(width, f32::INFINITY),
            size: Pixels(15.0),
            line_height: LineHeight::Relative(1.45),
            font: Font::DEFAULT,
            align_x: Alignment::Left,
            align_y: iced::alignment::Vertical::Top,
            shaping: Shaping::Advanced,
            wrapping: Wrapping::WordOrGlyph,
        })
    }

    type Paragraph = <iced::Renderer as iced::advanced::text::Renderer>::Paragraph;

    #[test]
    fn long_mixed_message_wraps_naturally_at_narrow_width() {
        // A long message with emoji near wrap boundaries: the whole thing
        // must flow as ONE paragraph, so text and emoji share lines and
        // wrap at word boundaries exactly like a plain message.
        let message = "the quick brown fox 😀 jumps over the lazy dog 🍕 and \
                       keeps running through the forest 🌲 until the sun \
                       sets behind the mountain 🏔️ and the stars come out ⭐";
        let spans = vec![Span::<(), Font>::new(message)];
        let plain = paragraph_with_spans(spans, 130.0);
        let plain_lines = plain.buffer().layout_runs().count();

        // Same text with three emoji as placeholder spans.
        let emoji_spans = vec![
            Span::<(), Font>::new("the quick brown fox "),
            Span::<(), Font>::new(EMOJI_PLACEHOLDER).color(Color::TRANSPARENT),
            Span::<(), Font>::new(" jumps over the lazy dog "),
            Span::<(), Font>::new(EMOJI_PLACEHOLDER).color(Color::TRANSPARENT),
            Span::<(), Font>::new(" and keeps running through the forest "),
            Span::<(), Font>::new(EMOJI_PLACEHOLDER).color(Color::TRANSPARENT),
            Span::<(), Font>::new(
                " until the sun sets behind the mountain and \
                                   the stars come out",
            ),
        ];
        let emoji = paragraph_with_spans(emoji_spans, 130.0);
        let emoji_lines = emoji.buffer().layout_runs().count();

        // Natural wrapping: both produce a comparable number of lines (the
        // emoji add width, so the emoji version may have one more line —
        // never a huge inflation like one line per emoji).
        assert!(
            (emoji_lines as i64 - plain_lines as i64).abs() <= 1,
            "emoji must not force every emoji onto its own row: \
             plain {plain_lines} lines vs emoji {emoji_lines} lines"
        );

        // No blank lines: total height is the line count × the exact same
        // line height as plain text.
        let line_height = 21.75; // 1.45 × 15
        assert!(
            (emoji.min_bounds().height - emoji_lines as f32 * line_height).abs() < 1.0,
            "emoji paragraph height {} should be {emoji_lines} lines × {line_height} \
             (no blank lines)",
            emoji.min_bounds().height
        );
    }

    #[test]
    fn every_emoji_placeholder_lands_on_one_line() {
        // After wrapping, each placeholder span must report exactly one
        // rectangle: the emoji box is atomic and never splits across lines.
        let spans = vec![
            Span::<(), Font>::new("aaa "),
            Span::<(), Font>::new(EMOJI_PLACEHOLDER).color(Color::TRANSPARENT),
            Span::<(), Font>::new(" bbb "),
            Span::<(), Font>::new(EMOJI_PLACEHOLDER).color(Color::TRANSPARENT),
            Span::<(), Font>::new(" ccc"),
        ];
        let para = paragraph_with_spans(spans, 60.0);
        for i in 0..2 {
            let bounds = para.span_bounds(i * 2 + 1);
            assert_eq!(
                bounds.len(),
                1,
                "emoji placeholder {i} must stay on a single line, got {bounds:?}"
            );
        }
    }

    // ── BORU-TWEMOJI-21: complex-sequence plan coverage ────────────────
    //
    // The parser tests (emoji/parser.rs) pin fragment boundaries; these
    // tests pin the artwork plan a chat bubble would render from those
    // fragments — mixed complex emoji with punctuation, and a multi-
    // codepoint ZWJ family staying ONE Svg artwork.

    #[test]
    fn plan_mixed_complex_emoji_with_punctuation() {
        let r = super::super::renderer::TwemojiRenderer;
        // Plain text + punctuation + a flag pair + skin tone + symbol +
        // celebration: the plan alternates Text/Svg exactly like the
        // parser's fragments, in input order. Note the single spaces
        // between emoji are their own Text runs (no merging across an
        // emoji) — 9 pieces total.
        let input = "Status: ✅ 🇮🇪🇦🇺 👍🏽 — done! 🎉";
        let plan = plan_emoji_text(&r, input);
        assert_eq!(plan.len(), 9, "plan mismatch: {plan:?}");
        let expected = [
            (Some("Status: "), None),
            (None, Some("2705")),
            (Some(" "), None),
            (None, Some("1f1ee-1f1ea")),
            (None, Some("1f1e6-1f1fa")),
            (Some(" "), None),
            (None, Some("1f44d-1f3fd")),
            (Some(" — done! "), None),
            (None, Some("1f389")),
        ];
        for (i, (text, key)) in expected.iter().enumerate() {
            match (&plan[i], text, key) {
                (EmojiTextArtwork::Text(t), Some(expected_text), None) => {
                    assert_eq!(*t, *expected_text, "Text piece {i}");
                }
                (EmojiTextArtwork::Svg { key: k, .. }, None, Some(expected_key)) => {
                    assert_eq!(k, expected_key, "Svg piece {i} key");
                }
                (piece, text, key) => {
                    panic!("piece {i} mismatch: got {piece:?}, expected text {text:?} key {key:?}")
                }
            }
        }
        // Every Svg artwork carries the original Unicode grapheme.
        let unicode_pieces: Vec<&str> = plan
            .iter()
            .filter_map(|item| match item {
                EmojiTextArtwork::Svg { unicode, .. } => Some(*unicode),
                _ => None,
            })
            .collect();
        assert_eq!(unicode_pieces, vec!["✅", "🇮🇪", "🇦🇺", "👍🏽", "🎉"]);
        // Roundtrip: the artwork plan reproduces the input byte-for-byte.
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
    fn plan_family_zwj_sequence_is_one_svg_artwork() {
        let r = super::super::renderer::TwemojiRenderer;
        // 👨👩👧👦 = U+1F468 ZWJ U+1F469 ZWJ U+1F467 ZWJ U+1F466 — the
        // longest sequence in the vendored set must stay ONE artwork whose
        // Unicode span is the whole 7-codepoint grapheme (a naive
        // char-based renderer would emit four separate images here).
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        let plan = plan_emoji_text(&r, family);
        assert_eq!(plan.len(), 1, "plan mismatch: {plan:?}");
        match &plan[0] {
            EmojiTextArtwork::Svg { unicode, key, .. } => {
                assert_eq!(*unicode, family, "whole ZWJ family must stay one artwork");
                assert_eq!(*key, "1f468-200d-1f469-200d-1f467-200d-1f466");
            }
            other => panic!("expected Svg artwork, got {other:?}"),
        }
    }
}
