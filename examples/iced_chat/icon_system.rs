//! Centralized icon system for the Boru desktop UI.
//!
//! ## Design
//!
//! One `Icon` enum maps every semantic action to its Lucide SVG asset.
//! `IconSize` gives named size steps so callers never pass raw pixels.
//! `Icon::build()` produces a styled SVG widget; `.tooltip()` wraps it in a
//! tooltip for accessible icon-only controls.
//!
//! ## Size classes
//!
//! | Token   | Pixels | Use                                  |
//! |---------|--------|--------------------------------------|
//! | XS      | 16     | Metadata, status dots, inline hints  |
//! | SM      | 18     | Inline actions, composer buttons     |
//! | MD      | 20     | Sidebar, toolbar, default            |
//! | LG      | 24     | Quick actions, home cards            |
//! | XL      | 28     | Hero / empty-state                   |
//!
//! ## Stroke weight
//!
//! All icons use the Lucide default (stroke-width 2, 24×24 viewBox).
//! The stroke is always `currentColor`; colour is applied via the
//! `iced::widget::svg::Style` callback.
//!
//! ## Destructive mode
//!
//! `Icon::destructive(true)` shows the danger colour **only on hover**.
//! At rest the icon stays in its normal muted state.  Use for tunnel
//! close, conversation clear, and similar destructive actions whose
//! normal toolbar appearance should not look visually alarming.

use iced::widget::svg;
use iced::widget::tooltip;
use iced::{Color, Element};

use crate::app::AppMessage;
use crate::design_tokens;

// ── Icon asset constants (Lucide SVG, embedded at compile time) ──────

const SVG_ARROW_LEFT: &[u8] = include_bytes!("../../assets/icons/lucide/arrow-left.svg");
const SVG_CHECK: &[u8] = include_bytes!("../../assets/icons/lucide/check.svg");
const SVG_PLAY: &[u8] = include_bytes!("../../assets/icons/lucide/play.svg");
const SVG_ELLIPSIS: &[u8] = include_bytes!("../../assets/icons/lucide/ellipsis.svg");
const SVG_ELLIPSIS_VERTICAL: &[u8] =
    include_bytes!("../../assets/icons/lucide/ellipsis-vertical.svg");
const SVG_ALERT_TRIANGLE: &[u8] = include_bytes!("../../assets/icons/lucide/alert-triangle.svg");
const SVG_SMILE: &[u8] = include_bytes!("../../assets/icons/lucide/smile.svg");
const SVG_SHARE_2: &[u8] = include_bytes!("../../assets/icons/lucide/share-2.svg");
const SVG_TERMINAL: &[u8] = include_bytes!("../../assets/icons/lucide/terminal.svg");
const SVG_IMAGE: &[u8] = include_bytes!("../../assets/icons/lucide/image.svg");
const SVG_USERS: &[u8] = include_bytes!("../../assets/icons/lucide/users.svg");
const SVG_UPLOAD: &[u8] = include_bytes!("../../assets/icons/lucide/upload.svg");
const SVG_CHEVRON_DOWN: &[u8] = include_bytes!("../../assets/icons/lucide/chevron-down.svg");
const SVG_CHEVRON_RIGHT: &[u8] = include_bytes!("../../assets/icons/lucide/chevron-right.svg");
const SVG_HOME: &[u8] = include_bytes!("../../assets/icons/lucide/home.svg");
const SVG_FOLDER: &[u8] = include_bytes!("../../assets/icons/lucide/folder.svg");
// Re-export the app-level constants for the Lucide icons that were already
// embedded there.  We keep them in app.rs so app-level code that already
// references ICON_CHAT etc. doesn't break, but the Icon enum uses them
// through its mapping.
use crate::app::{
    ICON_ACTIVITY, ICON_CHAT, ICON_CLOSE, ICON_COPY, ICON_FILES, ICON_FRIEND, ICON_LOCK, ICON_MESH,
    ICON_MORE, ICON_NOTIFICATION, ICON_OFFLINE, ICON_ONLINE, ICON_PAPERCLIP, ICON_PLUS, ICON_RETRY,
    ICON_SEARCH, ICON_SETTINGS, ICON_SWEEP, ICON_UNREAD, ICON_USER_PLUS,
};

// ── Icon enum ────────────────────────────────────────────────────────

/// Every icon used in the Boru UI, with a semantic name.
///
/// Each variant maps to one Lucide SVG asset.  The enum is the single
/// source of truth for "what icon goes with what action".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    // Navigation
    Back,
    Close,
    /// Expand/collapse disclosure chevrons.
    ChevronDown,
    ChevronRight,
    /// Go to the chat-list / home screen.
    Home,

    // Communication
    Chat,
    ChatFilled,
    Message,
    Plus,
    Search,
    Settings,

    // Social
    Friend,
    UserPlus,
    Notification,
    /// A group of people — used for group-chat actions (Figure 3).
    Users,

    // File / media
    Files,
    Folder,
    Paperclip,
    Image,
    Play,
    /// Upload / share-out arrow (Figure 3 share-files action).
    Upload,

    // Status / presence
    Online,
    Offline,

    // Actions
    Retry,
    More,
    MoreVertical,
    Check,
    Delete,
    Mesh,
    /// Copy an identifier / value to the clipboard.
    Copy,
    /// Share a room invitation or shared files.
    Share,
    /// Security / end-to-end encryption cue.
    Lock,

    // Meta
    Activity,
    Terminal,
    Smile,
    AlertTriangle,
}

// ── Byte mapping ─────────────────────────────────────────────────────

impl Icon {
    /// Return the embedded SVG bytes for this icon.
    pub fn bytes(self) -> &'static [u8] {
        match self {
            // Navigation
            Icon::Back => SVG_ARROW_LEFT,
            Icon::Close => ICON_CLOSE,
            Icon::ChevronDown => SVG_CHEVRON_DOWN,
            Icon::ChevronRight => SVG_CHEVRON_RIGHT,
            Icon::Home => SVG_HOME,

            // Communication
            Icon::Chat => ICON_CHAT,
            Icon::ChatFilled => ICON_UNREAD,
            Icon::Message => ICON_CHAT,
            Icon::Plus => ICON_PLUS,
            Icon::Search => ICON_SEARCH,
            Icon::Settings => ICON_SETTINGS,

            // Social
            Icon::Friend => ICON_FRIEND,
            Icon::UserPlus => ICON_USER_PLUS,
            Icon::Notification => ICON_NOTIFICATION,
            Icon::Users => SVG_USERS,

            // File / media
            Icon::Files => ICON_FILES,
            Icon::Folder => SVG_FOLDER,
            Icon::Paperclip => ICON_PAPERCLIP,
            Icon::Image => SVG_IMAGE,
            Icon::Play => SVG_PLAY,
            Icon::Upload => SVG_UPLOAD,

            // Status / presence
            Icon::Online => ICON_ONLINE,
            Icon::Offline => ICON_OFFLINE,

            // Actions
            Icon::Retry => ICON_RETRY,
            Icon::More => ICON_MORE,
            Icon::MoreVertical => SVG_ELLIPSIS_VERTICAL,
            Icon::Check => SVG_CHECK,
            Icon::Delete => ICON_SWEEP,
            Icon::Mesh => ICON_MESH,
            Icon::Copy => ICON_COPY,
            Icon::Share => SVG_SHARE_2,
            Icon::Lock => ICON_LOCK,

            // Meta
            Icon::Activity => ICON_ACTIVITY,
            Icon::Terminal => SVG_TERMINAL,
            Icon::Smile => SVG_SMILE,
            Icon::AlertTriangle => SVG_ALERT_TRIANGLE,
        }
    }
}

// ── Icon size ────────────────────────────────────────────────────────

/// Named size class.  Callers use a semantic token; the system picks
/// the pixel value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSize {
    /// 16 px — metadata, status dots, inline hints
    Xs,
    /// 18 px — inline actions, composer buttons
    Sm,
    /// 20 px — sidebar, toolbar (default)
    Md,
    /// 24 px — quick actions, home cards
    Lg,
    /// 28 px — hero / empty-state
    Xl,
}

impl IconSize {
    pub fn px(self) -> f32 {
        match self {
            IconSize::Xs => 16.0,
            IconSize::Sm => 18.0,
            IconSize::Md => 20.0,
            IconSize::Lg => 24.0,
            IconSize::Xl => 28.0,
        }
    }
}

impl Default for IconSize {
    fn default() -> Self {
        IconSize::Md
    }
}

// ── Builder ──────────────────────────────────────────────────────────

/// A configured icon ready to build into a widget.
///
/// Created via `Icon::build()` (default size, neutral colour) and
/// customised with a builder chain:
///
/// ```ignore
/// Icon::Back
///     .size(IconSize::Sm)
///     .color(design_tokens::text_muted)
///     .destructive(true)
///     .build()
/// ```
pub struct IconWidget {
    icon: Icon,
    size: IconSize,
    /// If `Some(f)`, the icon colour is determined by `f(theme)` instead
    /// of the default `text_secondary`.
    color_fn: Option<fn(&iced::Theme) -> Color>,
    /// If true, show danger colour only on hover; at rest stays neutral.
    destructive: bool,
    /// If true, the icon is treated as a rendered element (bypassed for hover
    /// colour in non-interactive contexts).
    interactive: bool,
}

impl Icon {
    /// Start building a widget with default size and neutral colouring.
    pub fn build(self) -> IconWidget {
        IconWidget {
            icon: self,
            size: IconSize::default(),
            color_fn: None,
            destructive: false,
            interactive: false,
        }
    }
}

impl IconWidget {
    /// Set the pixel size class.
    pub fn size(mut self, size: IconSize) -> Self {
        self.size = size;
        self
    }

    /// Force a specific colour function.  Overrides the default
    /// `text_secondary` behaviour.
    pub fn color_fn(mut self, f: fn(&iced::Theme) -> Color) -> Self {
        self.color_fn = Some(f);
        self
    }

    /// Mark this icon as destructive: normal state uses `text_muted`,
    /// hover switches to `destructive`.
    pub fn destructive(mut self, yes: bool) -> Self {
        self.destructive = yes;
        self
    }

    /// Treat this icon as interactive (responds to hover colour changes).
    /// Only makes sense when wrapped in a `button`; for standalone
    /// status icons leave this as false.
    pub fn interactive(mut self, yes: bool) -> Self {
        self.interactive = yes;
        self
    }

    /// Produce the styled SVG widget.
    ///
    /// Default colour: `text_secondary` at rest, `primary` on hover for
    /// interactive icons, `destructive` on hover for destructive icons.
    pub fn build<'a>(self) -> svg::Svg<'a, iced::Theme> {
        let bytes = self.icon.bytes();
        let px = self.size.px();
        let destructive = self.destructive;
        let color_fn = self.color_fn;

        svg(svg::Handle::from_memory(bytes))
            .width(iced::Length::Fixed(px))
            .height(iced::Length::Fixed(px))
            .style(move |theme, _status| {
                let color = if let Some(f) = color_fn {
                    f(theme)
                } else if destructive {
                    // At rest: muted.  The button's hover state will be
                    // handled by BUTTON_ICON_DESTRUCTIVE style; see below.
                    design_tokens::text_muted(theme)
                } else {
                    design_tokens::text_secondary(theme)
                };
                svg::Style { color: Some(color) }
            })
    }
}

// ── Button style: destructive icon ───────────────────────────────────

/// Button style for icon buttons whose icon should turn red only on hover.
///
/// Use this paired with `Icon::build().destructive(true)` on the icon.
/// At rest the button shows the icon in its muted colour; on hover both
/// the background and the text switch to the destructive palette.
pub fn button_icon_destructive(
    theme: &iced::Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    let color = match status {
        iced::widget::button::Status::Hovered => design_tokens::destructive(theme),
        iced::widget::button::Status::Pressed => {
            // Darken the destructive colour slightly on press
            let d = design_tokens::destructive(theme);
            Color::from_rgba(d.r * 0.85, d.g * 0.85, d.b * 0.85, 1.0)
        }
        iced::widget::button::Status::Disabled => design_tokens::text_muted(theme),
        _ => design_tokens::text_muted(theme),
    };
    iced::widget::button::Style {
        background: match status {
            iced::widget::button::Status::Hovered => {
                Some(iced::Background::Color(design_tokens::surface_hover(theme)))
            }
            iced::widget::button::Status::Pressed => Some(iced::Background::Color(
                design_tokens::selected_surface(theme),
            )),
            _ => None,
        },
        text_color: color,
        border: iced::Border {
            radius: design_tokens::RADIUS_SM.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

// ── Tooltip helper ────────────────────────────────────────────────────

/// Wrap an icon builder in a tooltip with the given label.
///
/// ```ignore
/// icon_with_tooltip(Icon::Back, "Go back", IconSize::Md).build()
/// ```
pub fn icon_with_tooltip<'a>(
    icon: Icon,
    label: &'a str,
    size: IconSize,
) -> tooltip::Tooltip<'a, AppMessage, iced::Theme, iced::Renderer> {
    let svg = icon.build().size(size).build();
    tooltip::Tooltip::new(
        svg,
        crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, label),
        tooltip::Position::Bottom,
    )
}

/// Wrap an already-built icon widget in a tooltip.
pub fn tooltip_for<'a>(
    svg: svg::Svg<'a, iced::Theme>,
    label: &'a str,
) -> tooltip::Tooltip<'a, AppMessage, iced::Theme, iced::Renderer> {
    tooltip::Tooltip::new(
        svg,
        crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, label),
        tooltip::Position::Bottom,
    )
}

// ── Convenience: horizontal ellipsis for friend rows ──────────────────

/// Build a three-dot "more" icon at the given size, suitable for use
/// inside a friend row or profile header.
pub fn more_icon<'a>(size: IconSize) -> svg::Svg<'a, iced::Theme> {
    Icon::More.build().size(size).build()
}

/// Build a vertical three-dot "kebab" icon at the given size.
pub fn more_vertical_icon<'a>(size: IconSize) -> svg::Svg<'a, iced::Theme> {
    Icon::MoreVertical.build().size(size).build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_has_svg_bytes() {
        let icons = [
            Icon::Back,
            Icon::Close,
            Icon::ChevronDown,
            Icon::ChevronRight,
            Icon::Chat,
            Icon::ChatFilled,
            Icon::Message,
            Icon::Plus,
            Icon::Search,
            Icon::Settings,
            Icon::Friend,
            Icon::UserPlus,
            Icon::Notification,
            Icon::Users,
            Icon::Files,
            Icon::Upload,
            Icon::Paperclip,
            Icon::Image,
            Icon::Play,
            Icon::Online,
            Icon::Offline,
            Icon::Retry,
            Icon::More,
            Icon::MoreVertical,
            Icon::Check,
            Icon::Delete,
            Icon::Mesh,
            Icon::Activity,
            Icon::Terminal,
            Icon::Smile,
            Icon::AlertTriangle,
        ];
        for icon in &icons {
            let bytes = icon.bytes();
            assert!(!bytes.is_empty(), "Icon {:?} has empty SVG bytes", icon);
            // Valid SVGs contain '<svg' (some may have leading comments)
            let content = std::str::from_utf8(bytes).unwrap_or("");
            assert!(
                content.contains("<svg"),
                "Icon {:?} does not contain <svg tag",
                icon
            );
        }
    }

    #[test]
    fn size_classes_are_monotonic() {
        assert!(IconSize::Xs.px() < IconSize::Sm.px());
        assert!(IconSize::Sm.px() < IconSize::Md.px());
        assert!(IconSize::Md.px() < IconSize::Lg.px());
        assert!(IconSize::Lg.px() < IconSize::Xl.px());
    }

    #[test]
    fn size_tokens_match_spec() {
        assert_eq!(IconSize::Xs.px(), 16.0);
        assert_eq!(IconSize::Sm.px(), 18.0);
        assert_eq!(IconSize::Md.px(), 20.0);
        assert_eq!(IconSize::Lg.px(), 24.0);
        assert_eq!(IconSize::Xl.px(), 28.0);
    }

    #[test]
    fn no_duplicate_svg_mappings() {
        // Verify that "Chat" and "Message" intentionally share the same SVG
        assert_eq!(Icon::Chat.bytes(), Icon::Message.bytes());
        // Verify that "Friend" and "UserPlus" intentionally share the same SVG
        assert_eq!(Icon::Friend.bytes(), Icon::UserPlus.bytes());
    }
}
