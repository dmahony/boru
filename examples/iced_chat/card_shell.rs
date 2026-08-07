//! Reusable card shell for the Boru home rail and dashboard cards
//! (Figure 3).
//!
//! A `CardShell` renders a titled card with an optional subtitle, optional
//! count badge, optional status pill ([`StatusBadgeKind`]), an optional
//! header action, and an optional footer, wrapping a body that is either
//! caller-provided content or a bounded, scrollable list:
//!
//! - **Header** — uppercase muted title (or sentence case via
//!   [`Self::title_case`]), optional muted subtitle below it, optional
//!   count badge, optional status pill, and an optional trailing ghost
//!   action button (default label "View all" via [`Self::on_view_all`],
//!   or any label via [`Self::header_action`]).
//! - **Body** — when [`Self::body`] is set, that element is rendered as-is
//!   with content-driven height (e.g. a mesh status block). When children
//!   are supplied instead, a scrollable list with a fixed maximum height
//!   (`max_height`, default [`DEFAULT_LIST_MAX_HEIGHT`]) so a long list
//!   scrolls instead of growing the dashboard without bound. When both are
//!   absent, the caller-provided `empty_message` is shown (optionally with
//!   a small icon via [`Self::empty_icon`], UI-HOME-16).
//! - **Footer** — optional element rendered below the body with a small top
//!   gap (e.g. a summary line or action row).
//!
//! All spacing, radii, borders, shadows, and colours come from
//! [`crate::design_tokens`] (`card_style`, `SPACE_*`, `RADIUS_CARD` and the
//! status palette); typography comes from the central
//! [`crate::fonts::TypeRole`] roles. Rows built by callers should target
//! [`CARD_ROW_HEIGHT`] (48 px) so single-line cards in the rail share the
//! same rhythm; two-line rows (Online Peers name + presence) use
//! [`PEER_ROW_HEIGHT`] (60 px) instead.
//!
//! The shell is intentionally data-agnostic: it never constructs list rows
//! and holds no sample data — the caller owns the children/body/footer.

use iced::widget::{button, container, Column, Row, Space};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::design_tokens;

/// Consistent height for a single list row inside a card shell (48 px).
///
/// Used by callers when building compact rows so Recent Activity and
/// Tunnels cards share the same row rhythm.
pub const CARD_ROW_HEIGHT: f32 = 48.0;

/// Consistent height for a two-line Online Peers row (60 px; plan band
/// 58–68 px).
///
/// Online-peer rows carry a display name plus a presence secondary line,
/// so they are taller than the single-line [`CARD_ROW_HEIGHT`] rows used
/// by the other rail cards. The visible-row cap in the Online Peers card
/// is computed from this token so the 6th peer scrolls.
pub const PEER_ROW_HEIGHT: f32 = 60.0;

/// Default fixed maximum height of the scrollable list body (px).
///
/// Below this many rows the list shrinks to its content; beyond it a vertical
/// scrollbar appears.
pub const DEFAULT_LIST_MAX_HEIGHT: f32 = 180.0;

/// Semantic status shown in a card's header pill.
///
/// Maps to the design-token status palette so every card that reports a
/// state (Healthy / Degraded / Offline / …) uses the same colours.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusBadgeKind {
    /// Neutral / informational — muted surface.
    Neutral,
    /// Success / healthy — green tint.
    Success,
    /// Warning / degraded — amber tint.
    Warning,
    /// Danger / offline / error — red tint.
    Danger,
}

/// Builder for a reusable card shell (Figure 3 right rail / dashboard card
/// foundation).
///
/// Example (list body):
/// ```
/// let shell = CardShell::new("Online Peers", rows)
///     .count(3)
///     .on_view_all(AppMessage::Noop)
///     .empty_message("No peers are online.")
///     .max_height(140.0)
///     .build(&theme);
/// ```
///
/// Example (arbitrary content body, subtitle, status badge and footer):
/// ```
/// let shell = CardShell::new("Mesh Health", vec![])
///     .title_case(false)
///     .subtitle("Current connection status")
///     .status_badge("Healthy", StatusBadgeKind::Success)
///     .header_action("View details", AppMessage::Noop)
///     .body(status_block)
///     .footer(summary_line)
///     .build(&theme);
/// ```
pub struct CardShell<'a, Message> {
    /// Header title — rendered uppercase per the Fig 3 rail look unless
    /// [`Self::title_case`] is disabled.
    title: String,
    /// Optional icon element rendered to the left of the title column.
    header_icon: Option<Element<'a, Message>>,
    /// Optional muted subtitle rendered below the title.
    subtitle: Option<String>,
    /// Optional count badge shown next to the title.
    count: Option<usize>,
    /// Optional total paired with `count` — when set the badge renders
    /// "online/total" (e.g. "3/12") instead of a bare number.
    count_total: Option<usize>,
    /// Optional semantic status pill (label + kind).
    status_badge: Option<(String, StatusBadgeKind)>,
    /// Optional trailing header action (label + message).
    header_action: Option<(String, Message)>,
    /// Message shown when `children` is empty and no `body` is set.
    empty_message: Option<String>,
    /// Optional small icon rendered to the left of the empty-state message
    /// (UI-HOME-16). The caller owns the element so the shell stays
    /// data-agnostic; muted colouring is the caller's responsibility.
    empty_icon: Option<Element<'a, Message>>,
    /// Fixed max height of the scrollable list body.
    max_height: f32,
    /// Vertical spacing between list rows.
    row_spacing: f32,
    /// When true (default) the header title is uppercased.
    title_case: bool,
    /// Arbitrary content body; when set it replaces the children list and
    /// the empty state, keeping the card's height content-driven.
    body: Option<Element<'a, Message>>,
    /// Optional element rendered below the body with a small top gap.
    footer: Option<Element<'a, Message>>,
    /// List rows rendered inside the bounded scrollable.
    children: Vec<Element<'a, Message>>,
    /// When true the header wraps onto a second line at narrow widths:
    /// title (and subtitle) on line one, badges and the action link on
    /// line two. Keeps headers readable without squeezing the title.
    compact_header: bool,
}

impl<'a, Message: Clone + 'a> CardShell<'a, Message> {
    /// Start a card shell with a title and its list rows.
    ///
    /// Pass an empty `children` vec together with
    /// [`Self::empty_message`] to render the empty state, or use
    /// [`Self::body`] for a single arbitrary content block.
    pub fn new(title: impl Into<String>, children: Vec<Element<'a, Message>>) -> Self {
        Self {
            title: title.into(),
            header_icon: None,
            subtitle: None,
            count: None,
            count_total: None,
            status_badge: None,
            header_action: None,
            empty_message: None,
            empty_icon: None,
            max_height: DEFAULT_LIST_MAX_HEIGHT,
            row_spacing: design_tokens::SPACE_2,
            title_case: true,
            body: None,
            footer: None,
            children,
            compact_header: false,
        }
    }

    /// Show an icon element at the start of the header, before the title.
    ///
    /// The caller owns the element (e.g. an [`crate::app::icon_svg`] mesh
    /// glyph), so the shell stays data-agnostic.
    pub fn header_icon(mut self, element: Element<'a, Message>) -> Self {
        self.header_icon = Some(element);
        self
    }

    /// Show a muted subtitle under the header title.
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Show a count badge in the header (e.g. online/total peers).
    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    /// Pair the count badge with a total so it reads "online/total".
    ///
    /// When set, the badge renders `"{count}/{total}"` (e.g. `3/12`);
    /// otherwise it renders just the count.
    pub fn count_total(mut self, total: usize) -> Self {
        self.count_total = Some(total);
        self
    }

    /// Show a semantic status pill in the header (e.g. Healthy / Degraded).
    pub fn status_badge(mut self, label: impl Into<String>, kind: StatusBadgeKind) -> Self {
        self.status_badge = Some((label.into(), kind));
        self
    }

    /// Add a labelled action button to the header (e.g. "View details").
    pub fn header_action(mut self, label: impl Into<String>, msg: Message) -> Self {
        self.header_action = Some((label.into(), msg));
        self
    }

    /// Add a "View all" action button to the header.
    pub fn on_view_all(mut self, msg: Message) -> Self {
        self.header_action = Some(("View all".to_string(), msg));
        self
    }

    /// Message shown when `children` is empty and no `body` is set.
    pub fn empty_message(mut self, message: impl Into<String>) -> Self {
        self.empty_message = Some(message.into());
        self
    }

    /// Show a small icon to the left of the empty-state message.
    ///
    /// UI-HOME-16: every list-oriented card renders its empty state as a
    /// small muted icon + muted supporting text so the rail never looks
    /// blank. The caller owns the element (e.g. an [`crate::app::icon_svg`]
    /// glyph tinted with `text_muted`), so the shell stays data-agnostic.
    pub fn empty_icon(mut self, element: Element<'a, Message>) -> Self {
        self.empty_icon = Some(element);
        self
    }

    /// Override the fixed maximum height of the scrollable list body.
    pub fn max_height(mut self, height: f32) -> Self {
        self.max_height = height;
        self
    }

    /// Override the vertical spacing between list rows.
    pub fn row_spacing(mut self, spacing: f32) -> Self {
        self.row_spacing = spacing;
        self
    }

    /// Render an arbitrary content body instead of the children list.
    ///
    /// The element sizes to its content, so cards using this API grow
    /// naturally instead of being clipped by a fixed list height.
    pub fn body(mut self, element: Element<'a, Message>) -> Self {
        self.body = Some(element);
        self
    }

    /// Render an optional footer below the body (with a small top gap).
    pub fn footer(mut self, element: Element<'a, Message>) -> Self {
        self.footer = Some(element);
        self
    }

    /// Toggle header-title uppercasing. Enabled by default (Fig 3 rail
    /// look); disable for sentence-case titles such as "Mesh Activity".
    pub fn title_case(mut self, enabled: bool) -> Self {
        self.title_case = enabled;
        self
    }

    /// Switch the header to the two-line compact layout (UI-HOME-15).
    ///
    /// On narrow content the single header row (icon + title + badges +
    /// action) can squeeze the title below a readable width. In compact
    /// mode the header becomes two rows: line one carries the icon and the
    /// title/subtitle column, line two carries the count/status badges and
    /// the action link. The body is unaffected.
    pub fn compact_header(mut self, enabled: bool) -> Self {
        self.compact_header = enabled;
        self
    }

    /// Build the card shell element.
    pub fn build(self, theme: &Theme) -> Element<'a, Message> {
        // Header: title column (title + optional subtitle), then count
        // badge, status badge, fill spacer and optional action button.
        let mut title_col = Column::new()
            .push(
                // Card title — card_title (IBM Plex Sans SemiBold 18);
                // uppercase + muted per the Fig 3 rail look.
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::CardTitle,
                    if self.title_case {
                        self.title.to_uppercase()
                    } else {
                        self.title.clone()
                    },
                )
                .color(design_tokens::text_muted(theme)),
            )
            // Card title → subtitle gap. UI-HOME-09: 4–8 px band (plan);
            // SPACE_4 is the shared-scale value (was SPACE_2, off the scale).
            .spacing(design_tokens::SPACE_4)
            .align_x(Alignment::Start);
        if let Some(subtitle) = self.subtitle {
            title_col = title_col.push(
                // Subtitle — supporting_text (IBM Plex Sans Regular 13).
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    subtitle,
                )
                .color(design_tokens::text_muted(theme)),
            );
        }

        // Badges + action link, shared by both the single-row and the
        // compact two-line header. The trailing fill spacer pushes the
        // action button to the right edge in both layouts.
        let mut badges_and_action = Row::new()
            .spacing(design_tokens::SPACE_8)
            .align_y(Alignment::Center);

        if let Some(count) = self.count {
            let label = match self.count_total {
                Some(total) => format!("{count}/{total}"),
                None => count.to_string(),
            };
            badges_and_action = badges_and_action.push(count_badge::<Message>(label));
        }

        if let Some((label, kind)) = self.status_badge {
            badges_and_action =
                badges_and_action.push(status_badge_element::<Message>(&label, kind, theme));
        }

        badges_and_action =
            badges_and_action.push(Space::new().width(Length::Fill).height(Length::Shrink));

        if let Some((label, msg)) = self.header_action {
            badges_and_action = badges_and_action.push(
                button(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    label,
                ))
                .on_press(msg)
                .padding([design_tokens::SPACE_2, design_tokens::SPACE_6])
                .style(view_all_button_style),
            );
        }

        // Compact header (UI-HOME-15): when the card's content width is
        // small, split the single header row into two lines so the title
        // never gets squeezed below a readable width. Line one = icon +
        // title/subtitle column (Fill so it wraps); line two = badges +
        // action link. The single-row layout keeps the approved Fig 3 look.
        let header: Element<'a, Message> = if self.compact_header {
            let mut line1 = Row::new()
                .spacing(design_tokens::SPACE_8)
                .align_y(Alignment::Center)
                .width(Length::Fill);
            if let Some(icon) = self.header_icon {
                line1 = line1.push(icon);
            }
            line1 = line1.push(title_col.width(Length::Fill));

            Column::new()
                .push(line1)
                .push(Space::new().height(Length::Fixed(design_tokens::SPACE_4)))
                .push(badges_and_action.width(Length::Fill))
                .spacing(0)
                .width(Length::Fill)
                .into()
        } else {
            let mut header = Row::new()
                // Horizontal gap between header elements (icon, title,
                // badges, action). UI-HOME-09: SPACE_8 shared-scale value.
                .spacing(design_tokens::SPACE_8)
                .align_y(Alignment::Center);

            if let Some(icon) = self.header_icon {
                header = header.push(icon);
            }

            header = header.push(title_col);
            header = header.push(badges_and_action);
            header.into()
        };

        // Body: caller body element, else empty state (with UI-04
        // empty-state typography), else a bounded scrollable list. The
        // fixed list height is what keeps many peers / activities from
        // growing the dashboard without bound; the explicit body element
        // stays content-driven.
        let body: Element<'a, Message> = if let Some(body_el) = self.body {
            body_el
        } else if self.children.is_empty() {
            if let Some(message) = self.empty_message {
                // Empty state (UI-HOME-16): a small muted icon (when the
                // caller supplies one via `empty_icon`) beside muted
                // supporting text. The text gets Fill width + word wrapping
                // so two-sentence copy reflows at narrow rail widths instead
                // of overflowing; vertical padding stays restrained
                // (SPACE_8) so an empty card never grows excessively tall.
                let mut empty_row = Row::new()
                    .align_y(Alignment::Center)
                    .width(Length::Fill)
                    .spacing(design_tokens::SPACE_8);
                if let Some(icon) = self.empty_icon {
                    empty_row = empty_row.push(icon);
                }
                empty_row = empty_row.push(
                    container(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            message,
                        )
                        .color(design_tokens::text_muted(theme))
                        .width(Length::Fill)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                    )
                    .width(Length::Fill),
                );
                container(empty_row)
                    .width(Length::Fill)
                    .padding([design_tokens::SPACE_8, 0.0])
                    .into()
            } else {
                Space::new()
                    .width(Length::Fill)
                    .height(Length::Shrink)
                    .into()
            }
        } else {
            crate::ui_components::gutter_scrollable(
                Column::with_children(self.children)
                    .spacing(self.row_spacing)
                    .width(Length::Fill),
            )
            .height(Length::Fixed(self.max_height))
            .width(Length::Fill)
            .into()
        };

        let mut content_col = Column::new()
            .push(header)
            // Card header → content gap. UI-HOME-09: 16–20 px band (plan);
            // SPACE_16 is the shared-scale value (was SPACE_6, off the scale).
            .push(Space::new().height(Length::Fixed(design_tokens::SPACE_16)))
            .push(body)
            .spacing(0)
            .width(Length::Fill);

        if let Some(footer_el) = self.footer {
            content_col = content_col
                // Body → footer gap. UI-HOME-09: shared-scale SPACE_8 (was
                // SPACE_6, off the scale).
                .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
                .push(footer_el);
        }

        container(content_col)
            // ~24 px internal padding on all sides (plan: 22–28 px band).
            .padding([design_tokens::SPACE_24, design_tokens::SPACE_24])
            .width(Length::Fill)
            .style(|t| design_tokens::card_style(t))
            .into()
    }
}

/// Count badge for the header — mirrors `ui_components::badge(.., BadgeKind::Accent)`
/// (primary_soft background, primary text) but takes an owned label so dynamic
/// values (including "online/total" pairs) never borrow from locals inside `build`.
fn count_badge<'a, Message: 'a>(label: String) -> Element<'a, Message> {
    container(
        // Count badge — metadata (IBM Plex Sans Regular 12); the primary
        // colour comes from the container style below.
        crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, label),
    )
    // UI-HOME-09: tokenise the vertical padding (was a raw 2.0 literal).
    .padding([design_tokens::SPACE_2, design_tokens::SPACE_8])
    .style(move |t| container::Style {
        background: Some(Background::Color(design_tokens::primary_soft(t))),
        text_color: Some(design_tokens::primary(t)),
        border: Border {
            radius: design_tokens::SPACE_12.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

/// Semantic status pill for the header — soft status tint background with
/// the strong status colour as text, so Healthy / Degraded / Offline use
/// the same palette as status dots elsewhere in the app.
fn status_badge_element<'a, Message: 'a>(
    label: &str,
    kind: StatusBadgeKind,
    theme: &Theme,
) -> Element<'a, Message> {
    let (bg, fg): (Color, Color) = match kind {
        StatusBadgeKind::Neutral => (
            design_tokens::surface_hover(theme),
            design_tokens::text_secondary(theme),
        ),
        StatusBadgeKind::Success => (
            design_tokens::success_soft(theme),
            design_tokens::color_success(theme),
        ),
        StatusBadgeKind::Warning => (
            design_tokens::warning_soft(theme),
            design_tokens::color_warning(theme),
        ),
        StatusBadgeKind::Danger => (
            design_tokens::destructive_soft(theme),
            design_tokens::color_danger(theme),
        ),
    };
    container(
        // Status pill — metadata (IBM Plex Sans Regular 12).
        crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, label.to_string()),
    )
    // UI-HOME-09: tokenise the vertical padding (was a raw 2.0 literal).
    .padding([design_tokens::SPACE_2, design_tokens::SPACE_8])
    .style(move |_t| container::Style {
        background: Some(Background::Color(bg)),
        text_color: Some(fg),
        border: Border {
            radius: design_tokens::SPACE_12.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

/// Ghost "View all" button — muted text, primary accent on hover/press.
fn view_all_button_style(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: None,
        text_color: match status {
            button::Status::Hovered => design_tokens::primary(theme),
            button::Status::Pressed => design_tokens::primary_pressed(theme),
            _ => design_tokens::text_secondary(theme),
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text;

    #[test]
    fn row_height_token_is_48px() {
        // The shared rail row height must stay exactly 48 px per the plan.
        assert_eq!(CARD_ROW_HEIGHT, 48.0);
    }

    #[test]
    fn peer_row_height_token_in_58_68_band() {
        // UI-HOME-07: two-line Online Peers rows (name + presence) target
        // the plan's 58–68 px band, not the 48 px single-line rhythm.
        assert!(
            (58.0..=68.0).contains(&PEER_ROW_HEIGHT),
            "PEER_ROW_HEIGHT must stay in the 58–68 px band, got {PEER_ROW_HEIGHT}"
        );
        assert!(
            PEER_ROW_HEIGHT > CARD_ROW_HEIGHT,
            "two-line peer rows must be taller than single-line rail rows"
        );
    }

    #[test]
    fn card_shell_default_max_height_is_bounded() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]);
        assert_eq!(shell.max_height, DEFAULT_LIST_MAX_HEIGHT);
        assert!(
            shell.max_height > 0.0,
            "max height must be finite and positive"
        );
    }

    #[test]
    fn card_shell_has_no_count_by_default() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]);
        assert_eq!(shell.count, None);
    }

    #[test]
    fn card_shell_stores_count() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]).count(7);
        assert_eq!(shell.count, Some(7));
    }

    #[test]
    fn card_shell_stores_count_total() {
        let shell: CardShell<'static, ()> =
            CardShell::new("Peers", vec![]).count(3).count_total(12);
        assert_eq!(shell.count, Some(3));
        assert_eq!(shell.count_total, Some(12));
    }

    #[test]
    fn card_shell_count_total_defaults_to_none() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]).count(3);
        assert_eq!(shell.count_total, None);
    }

    #[test]
    fn card_shell_build_count_total_badge_does_not_panic() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![])
            .count(3)
            .count_total(12)
            .empty_message("No peers online");
        let el = shell.build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn card_shell_stores_view_all_action() {
        let shell = CardShell::new("Peers", vec![]).on_view_all(());
        assert!(
            shell.header_action.is_some(),
            "View all action must be stored"
        );
        let (label, _) = shell.header_action.as_ref().unwrap();
        assert_eq!(label, "View all");
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]);
        assert!(shell.header_action.is_none(), "no action by default");
    }

    #[test]
    fn card_shell_stores_custom_header_action_label() {
        let shell = CardShell::new("Mesh", vec![]).header_action("View details", ());
        let (label, _) = shell.header_action.as_ref().expect("action stored");
        assert_eq!(label, "View details");
    }

    #[test]
    fn card_shell_stores_empty_message() {
        let shell: CardShell<'static, ()> =
            CardShell::new("Peers", vec![]).empty_message("No peers online");
        assert_eq!(shell.empty_message.as_deref(), Some("No peers online"));
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]);
        assert_eq!(shell.empty_message, None, "no empty message by default");
    }

    #[test]
    fn card_shell_stores_empty_icon() {
        let shell: CardShell<'static, ()> =
            CardShell::new("Peers", vec![]).empty_icon(text("icon").into());
        assert!(
            shell.empty_icon.is_some(),
            "empty-state icon must be stored when provided"
        );
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]);
        assert!(shell.empty_icon.is_none(), "no empty-state icon by default");
    }

    #[test]
    fn card_shell_build_empty_state_with_icon_does_not_panic() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![])
            .count(0)
            .empty_icon(text("icon").into())
            .empty_message("No peers are online right now. Connected peers will appear here.");
        let el = shell.build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn card_shell_build_empty_state_without_icon_does_not_panic() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![])
            .count(0)
            .empty_message("No peers are online right now. Connected peers will appear here.");
        let el = shell.build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn card_shell_stores_header_icon() {
        let shell: CardShell<'static, ()> =
            CardShell::new("Mesh Health", vec![]).header_icon(text("icon").into());
        assert!(
            shell.header_icon.is_some(),
            "header icon must be stored when provided"
        );
        let shell: CardShell<'static, ()> = CardShell::new("Mesh Health", vec![]);
        assert!(shell.header_icon.is_none(), "no header icon by default");
    }

    #[test]
    fn card_shell_build_with_header_icon_does_not_panic() {
        let shell: CardShell<'static, ()> = CardShell::new("Mesh Health", vec![])
            .header_icon(text("icon").into())
            .status_badge("Healthy", StatusBadgeKind::Success)
            .header_action("View details", ());
        let el = shell.build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn card_shell_stores_subtitle() {
        let shell: CardShell<'static, ()> =
            CardShell::new("Mesh Activity", vec![]).subtitle("Current connection status");
        assert_eq!(shell.subtitle.as_deref(), Some("Current connection status"));
        let shell: CardShell<'static, ()> = CardShell::new("Mesh Activity", vec![]);
        assert_eq!(shell.subtitle, None, "no subtitle by default");
    }

    #[test]
    fn card_shell_stores_status_badge() {
        let shell: CardShell<'static, ()> =
            CardShell::new("Mesh", vec![]).status_badge("Healthy", StatusBadgeKind::Success);
        let (label, kind) = shell.status_badge.as_ref().expect("badge stored");
        assert_eq!(label, "Healthy");
        assert_eq!(*kind, StatusBadgeKind::Success);
        let shell: CardShell<'static, ()> = CardShell::new("Mesh", vec![]);
        assert_eq!(shell.status_badge, None, "no status badge by default");
    }

    #[test]
    fn status_badge_kinds_cover_the_status_palette() {
        assert_eq!(
            [
                StatusBadgeKind::Neutral,
                StatusBadgeKind::Success,
                StatusBadgeKind::Warning,
                StatusBadgeKind::Danger,
            ]
            .len(),
            4
        );
    }

    #[test]
    fn card_shell_title_case_defaults_to_uppercase() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]);
        assert!(shell.title_case, "uppercase title is the rail default");
        let shell = shell.title_case(false);
        assert!(!shell.title_case);
    }

    #[test]
    fn card_shell_compact_header_defaults_off_and_builds() {
        // UI-HOME-15: the compact two-line header must be opt-in (default
        // keeps the approved single-row header) and must build without
        // panicking with every optional header element present.
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]);
        assert!(!shell.compact_header, "single-row header is the default");
        let theme = Theme::Light;
        let element = CardShell::new("Peers", vec![])
            .compact_header(true)
            .count(3)
            .count_total(12)
            .status_badge("Healthy", StatusBadgeKind::Success)
            .on_view_all(())
            .body(text("body").into())
            .build(&theme);
        // Building produces a non-empty element tree; no panic is the main
        // assertion (the two-line layout must not assume a header action).
        let _ = element;
        let minimal = CardShell::<()>::new("Empty", vec![])
            .compact_header(true)
            .body(text("body").into())
            .build(&theme);
        let _ = minimal;
    }

    #[test]
    fn card_shell_stores_children() {
        let children: Vec<Element<'static, ()>> = vec![text("a").into(), text("b").into()];
        let shell = CardShell::new("Peers", children);
        assert_eq!(shell.children.len(), 2);
    }

    #[test]
    fn card_shell_stores_body_and_footer() {
        let body_el: Element<'static, ()> = text("body").into();
        let footer_el: Element<'static, ()> = text("footer").into();
        let shell = CardShell::new("Peers", vec![])
            .body(body_el)
            .footer(footer_el);
        assert!(shell.body.is_some(), "body must be stored");
        assert!(shell.footer.is_some(), "footer must be stored");
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]);
        assert!(shell.body.is_none(), "no body by default");
        assert!(shell.footer.is_none(), "no footer by default");
    }

    #[test]
    fn card_shell_build_empty_state_does_not_panic() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![])
            .count(0)
            .empty_message("No peers are online right now.");
        let el = shell.build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn card_shell_build_populated_does_not_panic() {
        let children: Vec<Element<'static, ()>> =
            (0..12).map(|i| text(format!("Row {i}")).into()).collect();
        let shell = CardShell::new("Activity", children)
            .count(12)
            .on_view_all(())
            .max_height(120.0);
        let el = shell.build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn card_shell_build_full_semantic_areas_does_not_panic() {
        // Title + subtitle + count + status badge + header action + body
        // + footer: the complete dashboard-card foundation in one build.
        let body_el: Element<'static, ()> = text("Status block").into();
        let footer_el: Element<'static, ()> = text("Summary line").into();
        let shell = CardShell::new("Mesh Activity", vec![])
            .title_case(false)
            .subtitle("Current connection status")
            .count(4)
            .count_total(12)
            .status_badge("Degraded", StatusBadgeKind::Warning)
            .header_action("View details", ())
            .body(body_el)
            .footer(footer_el);
        let el = shell.build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn card_shell_build_with_body_ignores_empty_children() {
        let body_el: Element<'static, ()> = text("Custom body").into();
        let shell = CardShell::new("Peers", vec![])
            .empty_message("Should not render")
            .body(body_el);
        let el = shell.build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn card_shell_row_spacing_defaults_to_token() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]);
        assert_eq!(shell.row_spacing, design_tokens::SPACE_2);
    }

    #[test]
    fn card_shell_text_uses_type_role() {
        // UI-HOME-12: the shared shell's header title, count badge,
        // "View all" action and empty message must resolve through the
        // central TypeRole roles — never legacy Typography tokens. Only the
        // production part of this file is checked (the test's own messages
        // mention the same identifiers).
        let src = include_str!("card_shell.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        assert!(
            prod.contains("TypeRole::CardTitle"),
            "header title must use TypeRole::CardTitle (Plex SemiBold 18)"
        );
        assert!(
            prod.contains("TypeRole::Metadata"),
            "count/status badges must use TypeRole::Metadata (Plex Regular 12)"
        );
        assert!(
            prod.contains("TypeRole::ButtonLabel"),
            "\"View all\" action must use TypeRole::ButtonLabel (Plex SemiBold 14)"
        );
        assert!(
            prod.contains("TypeRole::SupportingText"),
            "subtitle + empty message must use TypeRole::SupportingText (Plex Regular 13)"
        );
        assert!(
            !prod.contains("Typography::"),
            "card shell must not use legacy Typography tokens"
        );
    }

    #[test]
    fn card_shell_spacing_uses_the_shared_scale() {
        // UI-HOME-09: the shared dashboard-card shell must use the plan's
        // shared spacing scale (4, 8, 12, 16, 20, 24, 32) for its structural
        // gaps — card title → subtitle 4–8 px, card header → content
        // 16–20 px, body → footer on-scale. Off-scale one-offs (SPACE_2 /
        // SPACE_6 structural gaps and raw 2.0 padding literals) are removed.
        let src = include_str!("card_shell.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        assert!(
            prod.contains(".spacing(design_tokens::SPACE_4)"),
            "card title → subtitle gap must use shared-scale SPACE_4 (4–8 px band)"
        );
        assert!(
            prod.contains("Length::Fixed(design_tokens::SPACE_16)"),
            "card header → content gap must use shared-scale SPACE_16 (16–20 px band)"
        );
        assert!(
            prod.contains("Length::Fixed(design_tokens::SPACE_8)"),
            "body → footer gap must use shared-scale SPACE_8"
        );
        assert!(
            !prod.contains(".padding([2.0,"),
            "badge paddings must use the SPACE_2 token, not a raw 2.0 literal"
        );
        // The structural header→body gap is 16 px — no SPACE_6 divider
        // between header and content.
        assert!(
            !prod.contains(".push(Space::new().height(Length::Fixed(design_tokens::SPACE_6)))"),
            "card shell must not use off-scale SPACE_6 for a structural gap"
        );
    }
}
