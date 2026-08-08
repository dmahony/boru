//! Reusable `BoruVideoFileCard` component for video file messages.
//!
//! This module owns the rendering of a video-file card in the chat log.
//! It is deliberately decoupled from the generic download-progress card
//! (image/file attachments still render through
//! [`crate::download_progress_view`]).
//!
//! The card is structured in four sections, mirroring the VIDCARD spec:
//!
//! - **Header** — compact transfer-state badge, video icon, single-line
//!   truncated filename (full name in a tooltip), format label, and an
//!   overflow menu for secondary actions.
//! - **Media frame** — bounded poster or the active inline player, a play
//!   overlay (only when ready), and the playback-error panel when a live
//!   player failed to open the file.
//! - **Status and metadata** — transfer/playback status, sender, size and
//!   speed (real values only; unavailable metadata is hidden).
//! - **Actions** — state-appropriate buttons (Download / Pause / Resume /
//!   Cancel / Retry / Play / Open File / Open Folder / Re-share / Remove)
//!   using the VIDCARD-13 hierarchy: green filled primary, light bordered
//!   secondary, destructive text for removal.
//!
//! The component is stateless: it renders a [`DownloadAttachment`] given
//! the live inline-player context owned by `app.rs`. All state transitions
//! and file-transfer logic remain in the parent app — this module only
//! composes design-system widgets.
//!
//! Supported real states (mapped from [`DownloadState`]):
//! downloading, download complete, ready to play, playing, paused,
//! transfer failed, file unavailable / deleted local file, re-shared file,
//! and outgoing shared file.

use iced::widget::text::Wrapping;
use iced::widget::{self, button, container, tooltip, Column, Row};
use iced::{Alignment, Color, Length};
#[cfg(feature = "video-playback")]
use iced_video_player::{Video, VideoPlayer};

use super::app::{
    accent_green, color_error, text_system, SPACE_10, SPACE_12, SPACE_16, SPACE_2, SPACE_20,
    SPACE_24, SPACE_4, SPACE_6, SPACE_8,
};
use super::app::{AppMessage, DownloadAttachment, DownloadState};
use super::download_progress_view::{
    action_buttons, active_download_detail, error_slot_height, failure_block, file_type_icon_element,
    file_type_icon_element_with_tooltip, fixed_slot, human_size, progress_section, resolve_theme,
    secondary_button, state_badge_color, DETAIL_SLOT_HEIGHT, METADATA_SLOT_HEIGHT, POLICY_SLOT_HEIGHT,
    PROGRESS_SLOT_HEIGHT,
};
use crate::design_tokens;
use crate::file_type_icon::FileTypeIconSize;
use crate::icon_system::{Icon, IconSize};
use crate::ui_components::OverflowMenu;

// ── Video presentation state ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoPresentationState {
    Remote,
    Downloading,
    Verifying,
    Ready,
    Failed,
    Missing,
}

pub(crate) fn video_presentation_state(attachment: &DownloadAttachment) -> VideoPresentationState {
    match &attachment.state {
        DownloadState::Ready { .. } | DownloadState::Cancelled => VideoPresentationState::Remote,
        DownloadState::Active { .. } | DownloadState::Paused { .. } => {
            VideoPresentationState::Downloading
        }
        DownloadState::Completed {
            saved_path: None, ..
        } => VideoPresentationState::Verifying,
        DownloadState::Completed {
            saved_path: Some(path),
            ..
        } if path.exists() => VideoPresentationState::Ready,
        DownloadState::Completed { .. } => VideoPresentationState::Missing,
        DownloadState::Shared { ref path, .. } if path.exists() => VideoPresentationState::Ready,
        DownloadState::Shared { .. } => VideoPresentationState::Missing,
        DownloadState::Failed { failure }
            if matches!(failure, super::app::DownloadFailure::FileRemoved) =>
        {
            VideoPresentationState::Missing
        }
        DownloadState::Failed { .. } => VideoPresentationState::Failed,
    }
}

// ── Aspect-ratio-aware media sizing ──────────────────────────────────

/// Layout class chosen from the media's intrinsic aspect ratio.
///
/// The ranges are deliberately tolerant (VIDCARD-05 spec): the class only
/// selects a bounded on-card footprint. The exact intrinsic ratio is always
/// preserved when the poster or player is rendered inside that frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaAspectClass {
    Portrait,
    Square,
    Landscape,
}

/// Classify a width/height ratio using the spec's tolerant ranges.
fn aspect_ratio_class(ratio: f32) -> MediaAspectClass {
    if ratio < 0.85 {
        MediaAspectClass::Portrait
    } else if ratio <= 1.15 {
        MediaAspectClass::Square
    } else {
        MediaAspectClass::Landscape
    }
}

/// Responsive band chosen from the measured chat timeline width (Task 15).
///
/// - `Wide` — full spec media caps; the card stays content-driven so a
///   portrait or square preview never forces the card to span the whole chat
///   width.
/// - `Medium` — media maximum dimensions are reduced (spec Task 15:
///   "reduce media maximum dimensions") and metadata/action rows wrap
///   naturally, while the card remains content-driven.
/// - `Narrow` — the card becomes 100% of the chat column; action buttons
///   wrap or stack; the header filename truncates against the remaining
///   space; media caps are reduced so previews stay inside the viewport and
///   nothing scrolls horizontally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardBand {
    Wide,
    Medium,
    Narrow,
}

/// Timeline width at or below which the card switches to a 100%-width
/// layout. Below the card's natural landscape footprint (~720 px frame plus
/// card padding) a content-driven card could not fit without overflowing, so
/// the card fills the column and its controls wrap/stack instead.
const NARROW_CARD_BREAKPOINT: f32 = 560.0;

/// Timeline width below which the media caps are scaled down (spec Task 15
/// medium behaviour).
const MEDIUM_CARD_BREAKPOINT: f32 = 780.0;

impl CardBand {
    fn of(width: f32) -> Self {
        if width <= NARROW_CARD_BREAKPOINT {
            CardBand::Narrow
        } else if width < MEDIUM_CARD_BREAKPOINT {
            CardBand::Medium
        } else {
            CardBand::Wide
        }
    }

    /// Media-cap scale for this band: Wide keeps the full spec caps; Medium
    /// and Narrow shrink them proportionally so previews stay bounded.
    fn media_scale(self) -> f32 {
        match self {
            CardBand::Wide => 1.0,
            CardBand::Medium => 0.85,
            CardBand::Narrow => 0.7,
        }
    }
}

/// One control component serves every aspect ratio. Only its density changes:
/// portrait (or very narrow) frames use the compact row so controls stay
/// inside the media frame without changing that frame's dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlLayout {
    Compact,
    Regular,
}

fn control_layout(dimensions: Option<(u32, u32)>, frame_width: f32) -> ControlLayout {
    let portrait = dimensions
        .filter(|(width, height)| *width > 0 && *height > 0)
        .map(|(width, height)| width < height)
        .unwrap_or(false);
    if portrait || frame_width < 360.0 {
        ControlLayout::Compact
    } else {
        ControlLayout::Regular
    }
}

/// Exact intrinsic aspect ratio, falling back to the spec's safe 16:9
/// default while video metadata is still unknown.
fn intrinsic_ratio(dimensions: Option<(u32, u32)>) -> f32 {
    dimensions
        .filter(|(width, height)| *width > 0 && *height > 0)
        .map(|(width, height)| width as f32 / height as f32)
        .unwrap_or(16.0 / 9.0)
}

/// Compute the largest ratio-exact box that fits inside `max_width ×
/// max_height` when starting from `start_width`: derive the height from the
/// width; if the height would exceed the cap, derive the width from the cap
/// instead. The result is always ratio-exact — never stretched, squashed or
/// cropped.
fn bounded_ratio_box(start_width: f32, max_width: f32, max_height: f32, ratio: f32) -> (f32, f32) {
    let mut frame_width = start_width.min(max_width);
    let mut frame_height = frame_width / ratio;
    if frame_height > max_height {
        frame_height = max_height;
        frame_width = frame_height * ratio;
    }
    (frame_width, frame_height)
}

/// Compute the bounded media-frame size for the given intrinsic dimensions
/// and responsive band.
///
/// Unknown dimensions fall back to a 16:9 widescreen default (the spec's safe
/// default while metadata loads). The returned `(width, height)` always
/// preserves the exact intrinsic aspect ratio; the class bounds only pick a
/// sensible on-card footprint so portrait videos do not dominate the chat and
/// landscape videos may use most or all of the card width. There is no fixed
/// 16:9 crop — the frame is ratio-exact in every normal case and `contain`
/// letterboxes only when an extreme ratio collides with both caps.
fn media_frame_size(dimensions: Option<(u32, u32)>, band: CardBand) -> (f32, f32) {
    let ratio = intrinsic_ratio(dimensions);
    let scale = band.media_scale();
    let (max_width, max_height) = match aspect_ratio_class(ratio) {
        // VIDCARD-06 landscape: the frame may use most or all of the card
        // width — the spec's typical 16:9 preview is 720×405 px — with a
        // ~500 px height cap so near-square or unusual landscape files
        // cannot dominate the chat. Very wide videos follow the width bound
        // and their exact ratio, producing a short, wide frame instead of an
        // excessive height; the media is contained (never cropped) inside it.
        MediaAspectClass::Landscape => (720.0 * scale, 500.0 * scale),
        MediaAspectClass::Square => (480.0 * scale, 520.0 * scale),
        MediaAspectClass::Portrait => (380.0 * scale, 520.0 * scale),
    };

    bounded_ratio_box(max_width, max_width, max_height, ratio)
}

/// Bounded, ratio-exact media-frame sizing (VIDCARD-08 / Task 15).
///
/// The frame is sized from the intrinsic dimensions (or the safe 16:9
/// default while metadata loads), the responsive band's media caps, and the
/// measured chat timeline width. The width is `min(available, cap)` and the
/// height is derived ratio-exact, capped by the band's height cap — so the
/// preview always stays inside the chat column, never overflows
/// horizontally, and a portrait preview never becomes unreasonably tall at
/// narrow widths. The sizing is concrete (Fixed lengths), so the poster and
/// the active player share exactly the same media box (Task 10) and both
/// shrink proportionally as the chat column narrows.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MediaFrameSizing {
    width: f32,
    height: f32,
}

impl MediaFrameSizing {
    fn new(dimensions: Option<(u32, u32)>, band: CardBand, available_width: f32) -> Self {
        let (nominal_width, nominal_height) = media_frame_size(dimensions, band);
        // Always stay within the measured chat column: when the column is
        // narrower than the nominal cap the frame starts from the available
        // width (shrinking proportionally); the height then derives
        // ratio-exact and is capped by the band's height cap so tall
        // portraits never overflow the viewport.
        let start_width = if available_width > 0.0 {
            available_width
        } else {
            nominal_width
        };
        let (width, height) = bounded_ratio_box(
            start_width,
            nominal_width,
            nominal_height,
            intrinsic_ratio(dimensions),
        );
        Self { width, height }
    }

    fn width(&self) -> Length {
        Length::Fixed(self.width)
    }

    fn height(&self) -> Length {
        Length::Fixed(self.height)
    }
}

/// Return the exact media-frame height used by the rendered video card.
///
/// The virtualized chat layout uses this same sizing path so its prefix sums
/// reserve the poster/player footprint before the card enters the window.
pub(crate) fn estimated_media_frame_height(
    dimensions: Option<(u32, u32)>,
    timeline_width: f32,
) -> f32 {
    MediaFrameSizing::new(
        dimensions,
        CardBand::of(timeline_width),
        (timeline_width - 2.0 * SPACE_24).max(0.0),
    )
    .height
}

/// Neutral dark media background (VIDCARD-08 / spec Tasks 8 & 11).
///
/// Video previews are framed on a fixed near-black neutral in BOTH themes so
/// letterboxed portrait/square content reads as a deliberate, polished media
/// surface rather than empty card space — the classic video-player
/// convention, and the same family as the play overlay / controls surfaces.
const MEDIA_FRAME_BACKGROUND: Color = Color::from_rgb(0.055, 0.06, 0.07);

/// Light neutral for on-media placeholder/error text — readable on
/// [`MEDIA_FRAME_BACKGROUND`] in both themes (the theme-aware `muted` token
/// is near-black in the light theme and would vanish on the dark frame).
const ON_MEDIA_TEXT: Color = Color::from_rgb(0.78, 0.80, 0.82);

/// Shared media-frame surface (VIDCARD-08 structure + VIDCARD-11 spec
/// styling): neutral dark background, thin subtle border, 12–14 px corner
/// radius — used identically by the poster frame, the placeholder frame and
/// the active player frame (Task 10 geometry). Overflow is clipped only at
/// this boundary (each media-frame container sets `.clip(true)`), so the
/// rounded corners never leak.
fn media_frame_style(_theme: &iced::Theme) -> widget::container::Style {
    widget::container::Style {
        background: Some(iced::Background::Color(MEDIA_FRAME_BACKGROUND)),
        border: iced::Border {
            color: MEDIA_FRAME_BORDER,
            width: 1.0,
            radius: MEDIA_FRAME_RADIUS.into(),
        },
        ..Default::default()
    }
}

/// Compact loading indicator shown while the poster or the inline player
/// prepares (VIDCARD-11). Rendered as a small translucent dark chip with
/// the Papirus video icon and a short label; there is no spinner widget in
/// iced 0.14, so this is a static-but-unmistakable loading affordance.
/// PAPIRUS-10: loading/thumbnail-failure states use the Papirus video icon
/// (the same central component the card header uses).
fn loading_indicator<'a>(
    attachment: &DownloadAttachment,
    dark_mode: bool,
) -> iced::Element<'a, AppMessage> {
    container(
        Column::new()
            .push(file_type_icon_element(
                &attachment.name,
                None,
                None,
                FileTypeIconSize::List,
                &resolve_theme(dark_mode),
            ))
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Preparing…")
                    .color(ON_MEDIA_TEXT),
            )
            .spacing(SPACE_4)
            .align_x(Alignment::Center),
    )
    .padding([SPACE_12, SPACE_16])
    .style(|_t| widget::container::Style {
        background: Some(iced::Background::Color(MEDIA_FRAME_OVERLAY_BG)),
        border: iced::Border {
            radius: SPACE_16.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

#[cfg(feature = "video-playback")]
fn format_media_time(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{:02}", seconds % 60)
    } else {
        format!("{minutes}:{:02}", seconds % 60)
    }
}

#[cfg(feature = "video-playback")]
fn media_icon_button(
    icon: Icon,
    label: &'static str,
    message: AppMessage,
) -> iced::Element<'static, AppMessage> {
    tooltip::Tooltip::new(
        crate::focusable_button::focusable_button(
            button(
                icon.build()
                    .size(IconSize::Sm)
                    .color_fn(|_| Color::WHITE)
                    .interactive(true)
                    .build(),
            )
            .on_press(message.clone())
            .padding(SPACE_6)
            .style(|_theme, status| widget::button::Style {
                background: matches!(status, widget::button::Status::Hovered | widget::button::Status::Pressed)
                    .then_some(iced::Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.16))),
                border: iced::Border {
                    radius: 20.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
            Some(message),
        )
        .on_key_press(|key, modifiers| {
            if modifiers.control() || modifiers.alt() || modifiers.logo() {
                return None;
            }
            use iced::keyboard::key::{Key, Named};
            match key {
                Key::Named(Named::ArrowLeft) => Some(AppMessage::InlineVideoSeekRelative(-5.0)),
                Key::Named(Named::ArrowRight) => Some(AppMessage::InlineVideoSeekRelative(5.0)),
                Key::Named(Named::ArrowUp) => Some(AppMessage::InlineVideoAdjustVolume(0.1)),
                Key::Named(Named::ArrowDown) => Some(AppMessage::InlineVideoAdjustVolume(-0.1)),
                Key::Character(value) if value.eq_ignore_ascii_case("m") => {
                    Some(AppMessage::InlineVideoToggleMute)
                }
                _ => None,
            }
        })
        .on_focus_change(|focused| AppMessage::InlineVideoControlsFocused(focused))
        .ring_radius(20.0),
        crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, label),
        tooltip::Position::Top,
    )
    .gap(SPACE_4)
    .into()
}

/// Compact relative label for the card's received/shared time, e.g.
/// `"2m ago"`, `"3h ago"`, falling back to an absolute short date
/// (`"Jan 5"`) for older entries. Real timestamps only — the caller
/// hides the time group entirely when the timestamp is `None`.
fn format_relative_time(timestamp_ms: i64, now_ms: i64) -> String {
    let elapsed_secs = (now_ms - timestamp_ms) / 1000;
    if elapsed_secs < 60 {
        "just now".to_string()
    } else if elapsed_secs < 3600 {
        format!("{}m ago", elapsed_secs / 60)
    } else if elapsed_secs < 86_400 {
        format!("{}h ago", elapsed_secs / 3600)
    } else {
        use chrono::TimeZone;
        chrono::Local
            .timestamp_millis_opt(timestamp_ms)
            .single()
            .map(|timestamp| timestamp.format("%b %d").to_string())
            .unwrap_or_default()
    }
}

/// Uppercase file extension used as the compact format label (e.g. "MP4").
fn file_format_label(name: &str) -> Option<String> {
    std::path::Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_uppercase())
}

// ── Header helpers ────────────────────────────────────────────────────

/// Maximum characters shown in the header filename before the stem is
/// collapsed with an ellipsis (the extension stays visible).
const HEADER_FILENAME_MAX_CHARS: usize = 56;

/// Hard width cap (px) for the header filename element. Together with
/// `.clip(true)` this guarantees a long filename can never widen the card.
const HEADER_FILENAME_MAX_WIDTH: f32 = 420.0;

// ── Media-frame styling (VIDCARD-11) ─────────────────────────────────
// The shared neutral-dark background and on-media text colours live with
// `MEDIA_FRAME_BACKGROUND` / `ON_MEDIA_TEXT` above (VIDCARD-08 landed the
// same spec direction first); this block adds the VIDCARD-11 deltas: the
// spec's 12–14 px radius, the thin subtle border, and the overlay/badge
// surfaces.

/// Corner radius of the media frame (spec Task 11: ~12–14 px).
const MEDIA_FRAME_RADIUS: f32 = 13.0;

/// Thin subtle border on the dark media frame. A faint light border keeps
/// the well visible against both light and dark card surfaces.
const MEDIA_FRAME_BORDER: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.10);

/// Semi-transparent dark surface used for the play overlay and the
/// loading indicator so they stay readable over any poster/video frame.
const MEDIA_FRAME_OVERLAY_BG: Color = Color::from_rgba(0.0, 0.0, 0.0, 0.62);

/// Diameter (px) of the circular play overlay button. Large but
/// restrained: clearly visible over the poster without dominating it.
const PLAY_OVERLAY_SIZE: f32 = 64.0;


/// Real transfer-state badge mapping for the card header.
///
/// Returns `(label, background, foreground)`. Only real states are shown;
/// nothing is invented. Positive transfer states use the green tint family;
/// failed / unavailable states use their own semantic tints so colour is
/// never the only cue.
fn header_badge(state: &DownloadState, theme: &iced::Theme) -> (String, Color, Color) {
    match state {
        DownloadState::Ready { .. } => (
            "Pending".to_string(),
            design_tokens::surface_hover(theme),
            design_tokens::text_secondary(theme),
        ),
        DownloadState::Active { .. } => (
            "Downloading".to_string(),
            design_tokens::primary_soft(theme),
            design_tokens::primary(theme),
        ),
        DownloadState::Paused { .. } => (
            "Paused".to_string(),
            design_tokens::primary_soft(theme),
            design_tokens::primary(theme),
        ),
        DownloadState::Completed {
            saved_path: None, ..
        } => (
            "Downloaded".to_string(),
            design_tokens::primary_soft(theme),
            design_tokens::primary(theme),
        ),
        DownloadState::Completed {
            saved_path: Some(path),
            ..
        } if path.exists() => (
            "Ready to play".to_string(),
            design_tokens::primary_soft(theme),
            design_tokens::primary(theme),
        ),
        DownloadState::Completed { .. } => (
            "Unavailable".to_string(),
            design_tokens::surface_hover(theme),
            design_tokens::text_muted(theme),
        ),
        DownloadState::Shared { ref path, .. } if path.exists() => (
            "Shared".to_string(),
            design_tokens::primary_soft(theme),
            design_tokens::primary(theme),
        ),
        DownloadState::Shared { .. } => (
            "Unavailable".to_string(),
            design_tokens::surface_hover(theme),
            design_tokens::text_muted(theme),
        ),
        DownloadState::Failed { failure }
            if matches!(failure, super::app::DownloadFailure::FileRemoved) =>
        {
            (
                "Unavailable".to_string(),
                design_tokens::surface_hover(theme),
                design_tokens::text_muted(theme),
            )
        }
        DownloadState::Failed { .. } => (
            "Failed".to_string(),
            design_tokens::destructive_soft(theme),
            design_tokens::destructive(theme),
        ),
        DownloadState::Cancelled => (
            "Cancelled".to_string(),
            design_tokens::surface_hover(theme),
            design_tokens::text_muted(theme),
        ),
    }
}

/// Compact tinted pill used for the header state badge.
fn header_badge_pill(
    label: &str,
    bg: Color,
    fg: Color,
) -> iced::widget::Container<'static, AppMessage> {
    container(
        crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, label.to_string())
            .color(fg),
    )
    .padding([SPACE_2, SPACE_8])
    .style(move |_t| widget::container::Style {
        background: Some(iced::Background::Color(bg)),
        border: iced::Border {
            radius: SPACE_10.into(),
            ..Default::default()
        },
        ..Default::default()
    })
}

/// Truncate a filename for single-line display while keeping the file
/// extension visible. Long names collapse to `stem…ext`; names without an
/// extension are tail-truncated with an ellipsis.
fn truncate_filename(name: &str, max_chars: usize) -> String {
    if name.chars().count() <= max_chars {
        return name.to_string();
    }
    if let Some(dot) = name.rfind('.') {
        if dot > 0 {
            let ext_budget = (max_chars / 3).max(4);
            let ext: String = name[dot..].chars().take(ext_budget).collect();
            let stem_budget = max_chars.saturating_sub(ext.chars().count() + 1);
            let stem: String = name[..dot].chars().take(stem_budget).collect();
            return format!("{stem}…{ext}");
        }
    }
    let mut out: String = name.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// One row of the header overflow menu: a left-aligned ghost button.
/// Each item reuses an existing app action; no new behaviour.
///
/// Keyboard-focusable (VIDCARD-17): the menu item is wrapped in
/// [`crate::focusable_button::FocusableButton`] so Tab reaches it and
/// Enter/Space activates it.
fn overflow_menu_item<'a>(label: &'a str, msg: AppMessage) -> iced::Element<'a, AppMessage> {
    crate::focusable_button::focusable_button(
        button(crate::fonts::type_role_text(
            crate::fonts::TypeRole::ButtonLabel,
            label,
        ))
        .on_press(msg.clone())
        .padding([SPACE_4, SPACE_8])
        .width(Length::Fill)
        .style(|t, status| {
            let background = match status {
                widget::button::Status::Hovered => design_tokens::surface_hover(t),
                widget::button::Status::Pressed => design_tokens::surface_selected(t),
                _ => Color::TRANSPARENT,
            };
            widget::button::Style {
                background: Some(iced::Background::Color(background)),
                text_color: design_tokens::text_primary(t),
                border: iced::Border {
                    radius: SPACE_6.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }),
        Some(msg),
    )
    .ring_radius(SPACE_6)
    .build()
}

/// Stable placeholder copy for the bounded media frame (VIDCARD-09).
///
/// While the async metadata probe is in flight — or while the sender's poster
/// blob is still being fetched (a thumbnail hash is present but no handle has
/// arrived) — the card shows a loading message at the safe default ratio. Once
/// the probe resolves (or fails) the text switches without changing the frame
/// geometry: the frame is always bounded via [`media_frame_size`], so
/// replacing the placeholder never causes a large layout jump or an
/// unrestricted-height frame.
fn media_placeholder_text(attachment: &DownloadAttachment) -> &'static str {
    if attachment.metadata_failed {
        "Preview unavailable"
    } else if attachment.metadata_loading || attachment.thumbnail_hash.is_some() {
        "Loading preview…"
    } else {
        "Preview available after download"
    }
}

// ── Reusable component ─────────────────────────────────────────────────

/// A reusable, stateless video-file card.
///
/// Construct via [`BoruVideoFileCard::new`] and render with
/// [`BoruVideoFileCard::view`]. The attachment is passed to `view` so the
/// component never borrows the card model beyond the render call — this
/// keeps the returned element's lifetime independent of the attachment
/// (matching the existing `download_progress_view` contract).
pub(crate) struct BoruVideoFileCard<'a> {
    entry_index: usize,
    dark_mode: bool,
    /// Whether this card's header overflow menu is currently expanded.
    /// The open/closed state lives in the parent app (stateless component);
    /// the card only renders the menu when told it is open.
    overflow_open: bool,
    /// Measured chat timeline width (px) supplied by the responsive wrapper
    /// in `view_chat_log`. Drives the card's responsive band (Task 15):
    /// the card fills the chat column at narrow widths, media caps shrink at
    /// medium/narrow widths, and the frame stays within the available space.
    timeline_width: f32,
    #[cfg(feature = "video-playback")]
    player: Option<&'a Video>,
    preparing: bool,
    /// Real chat-entry timestamp (Unix millis) of when the file was
    /// received/shared, used for the metadata row's time group. `None`
    /// hides the time group entirely — never fabricated.
    received_at_ms: Option<i64>,
    #[cfg(feature = "video-playback")]
    seek_position: Option<f32>,
    #[cfg(feature = "video-playback")]
    expanded: bool,
    #[cfg(feature = "video-playback")]
    controls_visible: bool,
    /// Keeps the lifetime parameter live in builds without the
    /// `video-playback` feature (where no field borrows `'a`).
    #[cfg(not(feature = "video-playback"))]
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> BoruVideoFileCard<'a> {
    /// Build the card for a chat entry. Player context is only meaningful
    /// with the `video-playback` feature (the non-feature build renders the
    /// bounded poster and routes Play to the OS open action).
    pub(crate) fn new(
        entry_index: usize,
        dark_mode: bool,
        overflow_open: bool,
        #[cfg(feature = "video-playback")] player: Option<&'a Video>,
        #[cfg(not(feature = "video-playback"))] _player: (),
        preparing: bool,
        #[cfg(feature = "video-playback")] seek_position: Option<f32>,
        #[cfg(feature = "video-playback")] expanded: bool,
        #[cfg(feature = "video-playback")] controls_visible: bool,
        received_at_ms: Option<i64>,
        timeline_width: f32,
    ) -> Self {
        Self {
            entry_index,
            dark_mode,
            overflow_open,
            timeline_width,
            #[cfg(feature = "video-playback")]
            player,
            preparing,
            received_at_ms,
            #[cfg(feature = "video-playback")]
            seek_position,
            #[cfg(feature = "video-playback")]
            expanded,
            #[cfg(feature = "video-playback")]
            controls_visible,
            #[cfg(not(feature = "video-playback"))]
            _marker: std::marker::PhantomData,
        }
    }

    /// Responsive band for this card, derived from the measured chat
    /// timeline width (Task 15).
    fn band(&self) -> CardBand {
        CardBand::of(self.timeline_width)
    }

    /// Inner content width available to the card: the measured chat timeline
    /// width minus the card's horizontal padding (`SPACE_24` on each side).
    /// Used to bound the media frame so it never overflows the chat column.
    fn inner_available_width(&self) -> f32 {
        (self.timeline_width - 2.0 * SPACE_24).max(0.0)
    }

    /// Render the full card.
    pub(crate) fn view(self, attachment: &DownloadAttachment) -> iced::Element<'a, AppMessage> {
        let theme = resolve_theme(self.dark_mode);
        let state = &attachment.state;
        let tone = state_badge_color(state, &theme);
        let muted = text_system(&theme);
        let error_color = color_error(&theme);

        let header = self.header(attachment, &theme);
        let media = self.media_frame(attachment, error_color);
        let status = self.status_metadata(attachment, &theme, tone, muted);
        let actions = self.actions(attachment);

        // Shape stability: the state-conditional sections (progress rows,
        // policy selector, failure block) render inside fixed-height slots,
        // so the card's outer box never collapses/expands when the state
        // changes.  The slot width is bounded to the media-frame width at
        // wide/medium (the card is content-driven there) so a long failure
        // message can never widen the card; at narrow the card fills the
        // column and the slot wraps at the card width.
        let media_sizing = MediaFrameSizing::new(
            attachment.poster_dimensions,
            self.band(),
            self.inner_available_width(),
        );
        let slot_width = if self.band() == CardBand::Narrow {
            Length::Fill
        } else {
            Length::Fixed(media_sizing.width)
        };
        let inner_width = if self.band() == CardBand::Narrow {
            self.inner_available_width()
        } else {
            media_sizing.width
        };

        let mut body = Column::new()
            .push(header)
            .push(
                // Centre the media frame within the card so a portrait or
                // square preview never hugs the left edge (VIDCARD-05).
                container(media).width(Length::Fill).center_x(Length::Fill),
            )
            .push(status)
            .push(actions)
            .spacing(SPACE_12);
        // Failure details — a fixed-height slot in EVERY state so the card
        // box never re-flows on failure (this task).  Failed fills it with
        // the bordered failure block; all other states reserve the same
        // space.
        let error_content: Option<iced::Element<'a, AppMessage>> = match &attachment.state {
            DownloadState::Failed { failure } => {
                Some(failure_block(failure, &theme, tone, muted, error_color))
            }
            _ => None,
        };
        body = body.push(
            container(fixed_slot(error_slot_height(inner_width), error_content))
                .width(slot_width),
        );
        body = body.spacing(SPACE_12);

        // VIDCARD-03 card surface: reuse the shared Boru card style —
        // soft white (theme-aware) background, thin neutral green-grey
        // border, RADIUS_CARD (16 px), very subtle shadow — with 20-24 px
        // internal padding.
        //
        // Task 15 responsive width: the card is content-driven (`Shrink`)
        // at wide/medium widths so a portrait or square preview never forces
        // the card to span the whole chat width, and becomes `Fill` (100% of
        // the chat column) at narrow widths. No hidden overflow here:
        // `.clip(true)` is only used at the media-frame boundary to respect
        // its rounded corners.
        let outer_width = if self.band() == CardBand::Narrow {
            Length::Fill
        } else {
            Length::Shrink
        };
        container(body)
            .width(outer_width)
            .padding([SPACE_20, SPACE_24])
            .style(|t| crate::design_tokens::card_style(t))
            .into()
    }

    // ── Header: badge + video icon + filename + format + overflow ────

    fn header(
        &self,
        attachment: &DownloadAttachment,
        theme: &iced::Theme,
    ) -> iced::Element<'a, AppMessage> {
        let state = &attachment.state;
        let (badge_label, badge_bg, badge_fg) = header_badge(state, theme);
        let muted = design_tokens::text_muted(theme);

        let badge = header_badge_pill(&badge_label, badge_bg, badge_fg);

        // PAPIRUS-10: the card header carries the central Papirus video icon
        // (Card, 32px) beside the filename — same component for every chat
        // surface, no per-screen extension maps.  The icon is informative
        // and surfaces the friendly type ("Video file") in a hover tooltip
        // (PAPIRUS-15 point 7); the filename remains the primary label.
        let video_icon = file_type_icon_element_with_tooltip(
            &attachment.name,
            None,
            None,
            FileTypeIconSize::Card,
            theme,
        );

        // Filename: single line, width-capped + clipped so a long name can
        // never widen the card. The tooltip exposes the full name and the
        // copy action in the overflow menu exposes it to the clipboard.
        // At narrow widths the filename becomes flexible (Task 15: filenames
        // truncate safely) — it fills the space left after the other header
        // items inside the 100%-width card, still capped by
        // HEADER_FILENAME_MAX_WIDTH; at wide/medium it stays content-driven.
        let narrow = self.band() == CardBand::Narrow;
        let display_name = truncate_filename(&attachment.name, HEADER_FILENAME_MAX_CHARS);
        let filename = container(
            crate::fonts::type_role_text(crate::fonts::TypeRole::BodyEmphasised, display_name)
                .color(design_tokens::text_primary(theme))
                .wrapping(Wrapping::None),
        )
        .width(if narrow { Length::Fill } else { Length::Shrink })
        .max_width(HEADER_FILENAME_MAX_WIDTH)
        .clip(true);
        let filename_tooltip = tooltip::Tooltip::new(
            filename,
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, attachment.name.clone())
                .wrapping(Wrapping::WordOrGlyph),
            tooltip::Position::Bottom,
        )
        .gap(SPACE_4);

        let mut title_row = Row::new()
            .push(badge)
            .push(video_icon)
            .push(filename_tooltip)
            .align_y(Alignment::Center)
            .spacing(SPACE_8);
        // At narrow widths the title row fills the card so the flexible
        // filename truncates against the remaining space instead of pushing
        // the card wider than the chat column.
        if narrow {
            title_row = title_row.width(Length::Fill);
        }

        if let Some(format) = file_format_label(&attachment.name) {
            title_row = title_row.push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, format)
                    .color(muted),
            );
        }

        title_row = title_row.push(
            tooltip::Tooltip::new(
                // VIDCARD-17: the kebab is an icon-only button, so it is
                // wrapped in FocusableButton for Tab traversal + Enter/Space
                // activation, and it carries the "More actions" tooltip as
                // its accessible name.
                crate::focusable_button::focusable_button(
                    OverflowMenu::build(
                        AppMessage::ToggleVideoCardMenu(self.entry_index),
                        false,
                        theme,
                    ),
                    Some(AppMessage::ToggleVideoCardMenu(self.entry_index)),
                )
                .ring_radius(crate::design_tokens::RADIUS_SM),
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "More actions"),
                tooltip::Position::Bottom,
            )
            .gap(SPACE_4),
        );

        let mut column = Column::new().push(title_row);
        if self.overflow_open {
            column = column.push(self.overflow_menu(attachment, theme));
        }
        column.spacing(SPACE_6).into()
    }

    /// Secondary actions shown under the header when the overflow menu is
    /// open. Each item reuses an existing app action; no new behaviour.
    fn overflow_menu(
        &self,
        attachment: &DownloadAttachment,
        theme: &iced::Theme,
    ) -> iced::Element<'a, AppMessage> {
        let state = &attachment.state;
        let name = attachment.name.clone();

        let mut menu = Column::new().spacing(SPACE_2);
        menu = menu.push(overflow_menu_item(
            "Copy filename",
            AppMessage::CopyToClipboard(name.clone()),
        ));
        menu = menu.push(overflow_menu_item(
            "Open downloads folder",
            AppMessage::OpenDownloadsFolder,
        ));

        match state {
            DownloadState::Completed {
                saved_path: Some(path),
                ..
            } if path.exists() => {
                menu = menu.push(overflow_menu_item(
                    "Open file",
                    AppMessage::OpenDownloadedFile(name),
                ));
                menu = menu.push(overflow_menu_item(
                    "Re-share",
                    AppMessage::ReshareFile(self.entry_index),
                ));
            }
            DownloadState::Shared { .. } => {
                menu = menu.push(overflow_menu_item(
                    "Open file",
                    AppMessage::OpenDownloadedFile(name),
                ));
                menu = menu.push(overflow_menu_item(
                    "Re-share",
                    AppMessage::ReshareFile(self.entry_index),
                ));
            }
            DownloadState::Active { .. }
            | DownloadState::Paused { .. }
            | DownloadState::Failed { .. }
            | DownloadState::Cancelled => {
                menu = menu.push(overflow_menu_item(
                    "Remove",
                    AppMessage::CancelDownloadAt(self.entry_index),
                ));
            }
            _ => {}
        }

        container(menu)
            .width(Length::Shrink)
            .padding(SPACE_4)
            .style(move |t| widget::container::Style {
                background: Some(iced::Background::Color(design_tokens::surface(t))),
                border: iced::Border {
                    color: design_tokens::border_muted(t),
                    width: 1.0,
                    radius: SPACE_8.into(),
                },
                ..Default::default()
            })
            .into()
    }

    // ── Media frame: poster or player + play overlay + error panel ────

    fn media_frame(
        &self,
        attachment: &DownloadAttachment,
        error_color: Color,
    ) -> iced::Element<'a, AppMessage> {
        let presentation = video_presentation_state(attachment);
        // Task 15: the frame is sized from the intrinsic dimensions (or the
        // safe 16:9 default), the responsive band's media caps and the
        // measured chat width — bounded so it never overflows the column and
        // never becomes unreasonably tall at narrow widths.
        let sizing = MediaFrameSizing::new(
            attachment.poster_dimensions,
            self.band(),
            self.inner_available_width(),
        );

        // Poster: the real thumbnail (contain, centred) or an honest
        // placeholder. While the poster is still being prepared (downloading
        // or verifying) show the loading indicator (VIDCARD-11).
        let poster: iced::Element<'static, AppMessage> =
            if let Some(ref handle) = attachment.thumbnail_handle {
                iced::widget::image(handle.clone())
                    // Contain: preserve the poster's exact intrinsic ratio,
                    // centred inside the fixed bounded frame — never stretch
                    // or crop. The frame size is computed from the measured
                    // chat width (Task 15), so the whole preview shrinks
                    // proportionally at medium/narrow window sizes instead of
                    // overflowing, while remaining ratio-exact and bounded.
                    .content_fit(iced::ContentFit::Contain)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            } else if matches!(
                presentation,
                VideoPresentationState::Downloading | VideoPresentationState::Verifying
            ) {
                container(loading_indicator(attachment, self.dark_mode))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .into()
            } else {
                // File-type placeholder while the poster is pending or when
                // extraction is not possible. A video with a thumbnail hash
                // is still being fetched; otherwise the poster will only
                // exist after the download completes. On-media text uses the
                // light `ON_MEDIA_TEXT` neutral because the media frame is a
                // fixed dark surface in both themes (VIDCARD-08).
                // PAPIRUS-10: the placeholder's main visual is the Papirus
                // video icon (Large, 48px), not the play glyph + "VIDEO" text.
                // Informative icon with a friendly-type hover tooltip
                // (PAPIRUS-15 point 7); the on-media subtitle stays the
                // primary content label.
                let subtitle = media_placeholder_text(attachment);
                container(
                    Column::new()
                        .push(file_type_icon_element_with_tooltip(
                            &attachment.name,
                            None,
                            None,
                            FileTypeIconSize::Large,
                            &resolve_theme(self.dark_mode),
                        ))
                        .push(
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Metadata,
                                subtitle,
                            )
                            .color(ON_MEDIA_TEXT),
                        )
                        .spacing(SPACE_4)
                        .align_x(Alignment::Center),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
            };
        let play_message = {
            #[cfg(feature = "video-playback")]
            {
                AppMessage::PlayInlineVideo(self.entry_index)
            }
            #[cfg(not(feature = "video-playback"))]
            {
                AppMessage::OpenDownloadedFile(attachment.name.clone())
            }
        };
        // VIDCARD-11 play overlay: large but restrained circular button with
        // strong contrast (white play glyph on a semi-transparent dark
        // circle), labelled "Play video" via the project's icon-button
        // Tooltip convention.
        //
        // VIDCARD-17 accessibility: iced 0.14 buttons have no
        // `operation::Focusable` impl and no keyboard handling, so the play
        // overlay is wrapped in [`crate::focusable_button::FocusableButton`].
        // That wrapper joins the app's Tab traversal, activates on
        // Enter/Space while focused, and draws a visible focus ring with the
        // same circular radius as the button. The inner button keeps its
        // mouse on_press for pointer users.
        let play_enabled = presentation == VideoPresentationState::Ready && !self.preparing;
        let play = tooltip::Tooltip::new(
            crate::focusable_button::focusable_button(
                button(
                    Icon::Play
                        .build()
                        .size(IconSize::Xl)
                        .color_fn(|_| Color::WHITE)
                        .interactive(true)
                        .build(),
                )
                .on_press_maybe(play_enabled.then_some(play_message.clone()))
                .padding([(PLAY_OVERLAY_SIZE - IconSize::Xl.px()) / 2.0; 2])
                .style(|_theme, _status| widget::button::Style {
                    background: Some(iced::Background::Color(MEDIA_FRAME_OVERLAY_BG)),
                    border: iced::Border {
                        radius: (PLAY_OVERLAY_SIZE / 2.0).into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                play_enabled.then_some(play_message),
            )
            .ring_radius(PLAY_OVERLAY_SIZE / 2.0),
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Play video"),
            tooltip::Position::Top,
        )
        .gap(SPACE_4);

        let error_preview = attachment.playback_error.as_ref().map(|error| {
            container(
                Column::new()
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::BodyEmphasised,
                            error.title(),
                        )
                        .color(error_color),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            error.message(),
                        )
                        .color(ON_MEDIA_TEXT),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            "The original attachment is still available below.",
                        )
                        .color(ON_MEDIA_TEXT),
                    )
                    .spacing(SPACE_4)
                    .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
        });
        let preview: iced::Element<'a, AppMessage> = {
            container(widget::stack![
                poster,
                error_preview.unwrap_or_else(|| {
                    if self.preparing {
                        // The inline player is still being prepared: show the
                        // loading indicator instead of the play overlay.
                        container(loading_indicator(attachment, self.dark_mode))
                            .center_x(Length::Fill)
                            .center_y(Length::Fill)
                    } else if presentation == VideoPresentationState::Ready {
                        container(play)
                            .center_x(Length::Fill)
                            .center_y(Length::Fill)
                    } else {
                        container(iced::widget::Space::new().width(0.0).height(0.0))
                    }
                })
            ])
            .width(sizing.width())
            .height(sizing.height())
            .clip(true)
            .style(media_frame_style)
            .into()
        };

        #[cfg(feature = "video-playback")]
        let preview = if attachment.playback_error.is_some() {
            preview
        } else if let Some(video) = self.player {
            let duration = video.duration();
            let position = video.position().min(duration);
            let duration_secs = duration.as_secs_f32().max(f32::EPSILON);
            let fraction = self
                .seek_position
                .unwrap_or((position.as_secs_f32() / duration_secs).clamp(0.0, 1.0));
            let layout = control_layout(attachment.poster_dimensions, sizing.width);
            let compact = layout == ControlLayout::Compact;
            let seek = iced::widget::slider(0.0..=1.0, fraction, AppMessage::InlineVideoSeekChanged)
                .on_release(AppMessage::InlineVideoSeekReleased)
                .step(0.001_f32)
                .style(|theme, status| {
                    let mut style = iced::widget::slider::default(theme, status);
                    let green = accent_green(theme);
                    style.rail.backgrounds.0 = green.into();
                    style.handle.background = green.into();
                    style.rail.width = match status {
                        iced::widget::slider::Status::Active => 3.5,
                        iced::widget::slider::Status::Hovered
                        | iced::widget::slider::Status::Dragged => 5.5,
                    };
                    style.handle.shape = iced::widget::slider::HandleShape::Circle {
                        radius: match status {
                            iced::widget::slider::Status::Active => 5.0,
                            iced::widget::slider::Status::Hovered
                            | iced::widget::slider::Status::Dragged => 6.5,
                        },
                    };
                    style
                })
                .width(Length::Fill);
            let controls = Column::new()
                .push(seek)
                .push(
                    Row::new()
                        .push(media_icon_button(
                            if video.paused() { Icon::Play } else { Icon::Pause },
                            if video.paused() { "Play video" } else { "Pause video" },
                            AppMessage::PlayInlineVideo(self.entry_index),
                        ))
                        .push(
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Metadata,
                                format!(
                                    "{} / {}",
                                    format_media_time(position),
                                    format_media_time(duration),
                                ),
                            )
                            .color(Color::WHITE),
                        )
                        .push({
                            let volume = video.volume() as f32;
                            let icon = if video.muted() {
                                Icon::VolumeX
                            } else if volume <= 0.01 {
                                Icon::VolumeX
                            } else if volume < 0.5 {
                                Icon::Volume1
                            } else {
                                Icon::Volume2
                            };
                            let volume_slider = iced::widget::slider(
                                0.0..=1.0,
                                volume,
                                AppMessage::InlineVideoSetVolume,
                            )
                            .step(0.01_f32)
                            .width(Length::Fixed(90.0));
                            tooltip::Tooltip::new(
                                media_icon_button(
                                    icon,
                                    if video.muted() { "Unmute" } else { "Mute" },
                                    AppMessage::InlineVideoToggleMute,
                                ),
                                volume_slider,
                                tooltip::Position::Top,
                            )
                            .gap(SPACE_4)
                        })
                        .push(media_icon_button(
                            Icon::More,
                            "More video actions",
                            AppMessage::InlineVideoToggleExpanded,
                        ))
                        .spacing(if compact { SPACE_2 } else { SPACE_6 })
                        .align_y(Alignment::Center),
                );
            // Task 10: the playing element occupies the exact same media box
            // as the poster — no layout jump when Play is pressed. The video
            // is contained (never stretched or cropped) and the controls
            // overlay the frame's bottom edge on the existing translucent
            // dark surface, so poster and player share width, height, aspect
            // ratio, border radius and position. Both use the same bounded
            // Task 15 sizing, so the player shrinks proportionally with the
            // measured chat width exactly like the poster.
            let video_element: iced::Element<'a, AppMessage> = iced::widget::mouse_area(
                container(
                    VideoPlayer::new(&video)
                        .content_fit(iced::ContentFit::Contain)
                        .on_end_of_stream(AppMessage::CloseInlineVideo)
                        .on_error(|_error| AppMessage::CloseInlineVideo),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
            )
            .on_press(AppMessage::PlayInlineVideo(self.entry_index))
            .on_enter(AppMessage::InlineVideoShowControls)
            .on_move(|_| AppMessage::InlineVideoShowControls)
            .into();

            let controls_bar = container(
                Column::new()
                    .push(controls)
                    .width(Length::Fill)
                    .spacing(SPACE_2),
            )
                .padding([SPACE_6, SPACE_12])
                .style(|_theme| widget::container::Style {
                    // The overlay is deliberately limited to the control
                    // footprint: transparent at its top, readable black at
                    // the bottom, never an opaque strip over the video.
                    background: Some(iced::Background::Gradient(iced::Gradient::Linear(
                        iced::gradient::Linear::new(iced::Radians(std::f32::consts::FRAC_PI_2))
                            .add_stop(0.0, Color::from_rgba(0.0, 0.0, 0.0, 0.0))
                            .add_stop(0.55, Color::from_rgba(0.0, 0.0, 0.0, 0.62))
                            .add_stop(1.0, Color::from_rgba(0.0, 0.0, 0.0, 0.84)),
                    ))),
                    ..Default::default()
                });

            let controls_overlay: iced::Element<'a, AppMessage> = if self.controls_visible {
                container(controls_bar)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_y(Alignment::End)
                    .into()
            } else {
                container(iced::widget::Space::new())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            };
            container(widget::stack![
                video_element,
                controls_overlay,
            ])
            .width(sizing.width())
            .height(sizing.height())
            .clip(true)
            .style(media_frame_style)
            .into()
        } else {
            preview
        };

        preview
    }

    // ── Status and metadata ───────────────────────────────────────────

    fn status_metadata(
        &self,
        attachment: &DownloadAttachment,
        theme: &iced::Theme,
        tone: Color,
        muted: Color,
    ) -> iced::Element<'a, AppMessage> {
        let state = &attachment.state;
        let presentation = video_presentation_state(attachment);

        // ── State line ────────────────────────────────────────────────
        // Prominent, real presentation state (e.g. "Ready to play").
        // VIDCARD-14: active downloads name the real source peer in the
        // status line ("Downloading from Duke") and paused downloads say
        // "Paused" instead of the generic downloading text.
        let status = if self.preparing {
            "Preparing video…".to_string()
        } else if let Some(player_status) = self.playback_status() {
            player_status
        } else {
            match state {
                DownloadState::Active { .. } if !attachment.source_peer.is_empty() => {
                    format!("Downloading from {}", attachment.source_peer)
                }
                DownloadState::Active { .. } => "Downloading video…".to_string(),
                DownloadState::Paused { .. } if !attachment.source_peer.is_empty() => {
                    format!("Paused — from {}", attachment.source_peer)
                }
                DownloadState::Paused { .. } => "Paused".to_string(),
                _ => match presentation {
                    VideoPresentationState::Ready => "Ready to play".to_string(),
                    VideoPresentationState::Downloading => "Downloading video…".to_string(),
                    VideoPresentationState::Verifying => "Verifying video…".to_string(),
                    VideoPresentationState::Failed => "Download failed".to_string(),
                    VideoPresentationState::Missing => {
                        "Local file missing · download again".to_string()
                    }
                    VideoPresentationState::Remote => "Static preview · download to play".to_string(),
                },
            }
        };
        // The active/paused status line is part of the green progress
        // treatment; paused snaps to the muted tone so colour is never the
        // only cue.  Other states keep the badge colour.
        let status_color = match state {
            DownloadState::Active { .. } => accent_green(theme),
            DownloadState::Paused { .. } => text_system(theme),
            _ => tone,
        };
        let mut column = Column::new().push(
            crate::fonts::type_role_text(
                crate::fonts::TypeRole::BodyEmphasised,
                format!("●  {status}"),
            )
            .color(status_color),
        );

        // ── Metadata groups (real values only; hidden when unavailable) ─
        // One wrapping muted line so the groups stack gracefully at narrow
        // widths, separated by quiet dividers.  While actively downloading
        // (or paused) with a known source, the peer is already named in the
        // status line, so the separate "From:" group is skipped to avoid
        // duplication.
        let status_carries_peer = matches!(
            state,
            DownloadState::Active { .. } | DownloadState::Paused { .. }
        ) && !attachment.source_peer.is_empty();
        let source_label = if status_carries_peer || attachment.source_peer.is_empty() {
            String::new()
        } else {
            format!("From: {}", attachment.source_peer)
        };
        let size_label = match state {
            DownloadState::Ready { total: Some(total) }
            | DownloadState::Active {
                total: Some(total), ..
            }
            | DownloadState::Paused {
                total: Some(total), ..
            }
            | DownloadState::Completed {
                total_size: Some(total),
                ..
            }
            | DownloadState::Shared {
                size: Some(total), ..
            } if *total > 0 => human_size(*total),
            _ => String::new(),
        };
        // Duration is only genuinely known while a live player is attached
        // (the transfer protocol does not carry a duration field).
        #[cfg(feature = "video-playback")]
        let duration_label = self.player.map(|video| format_media_time(video.duration()));
        #[cfg(not(feature = "video-playback"))]
        let duration_label: Option<String> = None;
        let time_label = self.received_at_ms.map(|received_at_ms| {
            let relative =
                format_relative_time(received_at_ms, chrono::Local::now().timestamp_millis());
            if attachment.source_peer.is_empty() {
                format!("Shared {relative}")
            } else {
                format!("Received {relative}")
            }
        });

        let mut groups: Vec<String> = Vec::new();
        if !source_label.is_empty() {
            groups.push(source_label);
        }
        if !size_label.is_empty() {
            groups.push(size_label);
        }
        if let Some(duration) = duration_label {
            groups.push(duration);
        }
        if let Some(time) = time_label {
            groups.push(time);
        }
        // Shape stability: the metadata line is a FIXED-height slot.  The
        // group list is partly state-dependent (size disappears on
        // Failed/Cancelled), so reserving the space keeps the card box
        // identical across every DownloadState.
        let groups_el: Option<iced::Element<'a, AppMessage>> = if groups.is_empty() {
            None
        } else {
            Some(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    groups.join("  ·  "),
                )
                .color(muted)
                .wrapping(Wrapping::Word)
                .width(Length::Fill)
                .into(),
            )
        };
        column = column.push(fixed_slot(METADATA_SLOT_HEIGHT, groups_el));

        // Progress bar + in-flight detail — fixed-height slots reserved in
        // every state; content only Active/Paused (shape stability).
        column = column.push(fixed_slot(
            PROGRESS_SLOT_HEIGHT,
            progress_section(state, self.dark_mode),
        ));
        // VIDCARD-14: bytes of total, percentage and transfer speed — only
        // where the transfer layer provides them (no invented estimates).
        // Active uses the green progress accent; paused uses the muted tone.
        let detail_el: Option<iced::Element<'a, AppMessage>> =
            active_download_detail(attachment).map(|detail| {
                let detail_color = if matches!(state, DownloadState::Paused { .. }) {
                    muted
                } else {
                    accent_green(theme)
                };
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, detail)
                    .color(detail_color)
                    .into()
            });
        column = column.push(fixed_slot(DETAIL_SLOT_HEIGHT, detail_el));

        column.spacing(SPACE_6).into()
    }

    #[cfg(feature = "video-playback")]
    fn playback_status(&self) -> Option<String> {
        let video = self.player?;
        Some(if video.paused() {
            "Paused".to_string()
        } else {
            "Playing".to_string()
        })
    }

    #[cfg(not(feature = "video-playback"))]
    fn playback_status(&self) -> Option<String> {
        None
    }

    // ── Actions ───────────────────────────────────────────────────────

    fn actions(&self, attachment: &DownloadAttachment) -> iced::Element<'a, AppMessage> {
        let state = &attachment.state;
        let name_str = attachment.name.clone();
        let mut column = Column::new();

        // VIDCARD-13: state-appropriate primary/secondary actions come from
        // the shared action_buttons helper (green filled primary, light
        // bordered secondary, destructive text for removal).
        let action_row = action_buttons(self.entry_index, attachment.kind, state, &name_str);
        // Shape stability: the action row is a fixed-height slot.  The
        // button count changes per state (1-5 buttons, wrapping at narrow
        // widths); reserving the space keeps the card box identical.
        let inner_width = if self.band() == CardBand::Narrow {
            self.inner_available_width()
        } else {
            MediaFrameSizing::new(
                attachment.poster_dimensions,
                self.band(),
                self.inner_available_width(),
            )
            .width
        };
        column = column.push(fixed_slot(
            super::download_progress_view::action_slot_height(inner_width),
            Some(action_row),
        ));

        // FS-26 overwrite-conflict policy: while the download is ready to
        // start, surface the policy that decides what happens when the
        // destination file already exists. Default is Keep Both — never
        // silently overwrite.  Reserved as a fixed slot in every state
        // (visibility preserved in Ready; shape stability everywhere).
        let policy = if matches!(state, DownloadState::Ready { .. }) {
            Some(super::download_progress_view::policy_selector(
                self.entry_index,
                attachment.overwrite_policy,
            ))
        } else {
            None
        };
        column = column.push(fixed_slot(POLICY_SLOT_HEIGHT, policy));

        if let Some(error) = attachment.playback_error.as_ref() {
            if error.retry_available() {
                column = column.push(iced::Element::<'_, AppMessage>::from(secondary_button(
                    None,
                    "Retry player",
                    AppMessage::PlayInlineVideo(self.entry_index),
                )));
            }
        }

        column.spacing(SPACE_6).into()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        aspect_ratio_class, file_format_label, format_relative_time, header_badge, intrinsic_ratio,
        media_frame_size, media_placeholder_text, truncate_filename, video_presentation_state,
        BoruVideoFileCard, CardBand, ControlLayout, MediaAspectClass, MediaFrameSizing, VideoPresentationState,
        control_layout,
        HEADER_FILENAME_MAX_CHARS, MEDIUM_CARD_BREAKPOINT, NARROW_CARD_BREAKPOINT,
    };
    use super::super::app::AppMessage;
    use iced::Length;
    use crate::app::{DownloadAttachment, DownloadFailure, DownloadState, TransferKind};
    use std::path::PathBuf;

    #[test]
    fn controls_use_one_responsive_component_for_all_shapes() {
        assert_eq!(control_layout(Some((1920, 1080)), 720.0), ControlLayout::Regular);
        assert_eq!(control_layout(Some((1080, 1080)), 480.0), ControlLayout::Regular);
        assert_eq!(control_layout(Some((1080, 1920)), 292.5), ControlLayout::Compact);
        assert_eq!(control_layout(Some((720, 1600)), 250.0), ControlLayout::Compact);
        assert_eq!(control_layout(Some((1920, 1080)), 359.0), ControlLayout::Compact);
    }

    #[test]
    fn control_density_does_not_change_media_geometry() {
        let portrait = MediaFrameSizing::new(Some((1080, 1920)), CardBand::Wide, 720.0);
        assert!((portrait.width / portrait.height - 1080.0 / 1920.0).abs() < 1e-6);
        assert_eq!(portrait.width, 292.5);
        assert_eq!(portrait.height, 520.0);
    }

    #[test]
    fn aspect_ratio_class_uses_tolerant_spec_ranges() {
        use MediaAspectClass::*;
        assert_eq!(aspect_ratio_class(0.84), Portrait);
        assert_eq!(aspect_ratio_class(0.85), Square);
        assert_eq!(aspect_ratio_class(1.0), Square);
        assert_eq!(aspect_ratio_class(1.15), Square);
        assert_eq!(aspect_ratio_class(1.16), Landscape);
    }

    #[test]
    fn relative_time_labels_use_real_elapsed_values() {
        let now_ms = 1_800_000_000_000_i64;
        assert_eq!(format_relative_time(now_ms - 5_000, now_ms), "just now");
        assert_eq!(format_relative_time(now_ms - 95_000, now_ms), "1m ago");
        assert_eq!(format_relative_time(now_ms - 2_400_000, now_ms), "40m ago");
        assert_eq!(format_relative_time(now_ms - 7_200_000, now_ms), "2h ago");
    }

    #[test]
    fn relative_time_falls_back_to_absolute_date_for_old_entries() {
        // 90 days before the reference instant is older than a day, so the
        // label must be an absolute short date rather than "Xh ago".
        let now_ms = 1_800_000_000_000_i64;
        let old_ms = now_ms - 90 * 86_400_000;
        let label = format_relative_time(old_ms, now_ms);
        assert!(
            label.chars().any(char::is_alphabetic),
            "expected an absolute date label, got {label:?}"
        );
        assert!(!label.contains("ago"), "got a relative label: {label:?}");
    }

    #[test]
    fn unknown_dimensions_fall_back_to_bounded_widescreen_default() {
        // No dimensions yet: safe 16:9 default at the landscape width bound.
        let (width, height) = media_frame_size(None, CardBand::Wide);
        assert_eq!(width, 720.0);
        assert!((height - 405.0).abs() < 0.01);
    }

    #[test]
    fn landscape_frame_preserves_exact_intrinsic_ratio() {
        // 16:9 fills the landscape width bound; the height derives from the
        // exact ratio (no fixed 16:9 crop, no stretch/squash).
        let (width, height) = media_frame_size(Some((3840, 2160)), CardBand::Wide);
        assert_eq!(width, 720.0);
        assert!((height - 405.0).abs() < 0.01);
        assert!((width / height - 3840.0 / 2160.0).abs() < 1e-6);
    }

    #[test]
    fn landscape_typical_hd_preview_matches_spec() {
        // VIDCARD-06 spec: a typical 16:9 preview is approximately
        // 720×405 px where space allows. 1280×720 derives exactly that.
        let (width, height) = media_frame_size(Some((1280, 720)), CardBand::Wide);
        assert_eq!(width, 720.0);
        assert!((height - 405.0).abs() < 0.01);
        assert!((width / height - 1280.0 / 720.0).abs() < 1e-6);
    }

    #[test]
    fn landscape_frame_caps_height_for_near_square_ratios() {
        // 4:3 landscape would exceed the ~500 px height cap at the full
        // landscape width, so the width derives down from the cap — the
        // result stays ratio-exact and never dominates the chat.
        let (width, height) = media_frame_size(Some((640, 480)), CardBand::Wide);
        assert_eq!(height, 500.0);
        assert!((width - 666.6667).abs() < 0.01);
        assert!((width / height - 640.0 / 480.0).abs() < 1e-6);
    }

    #[test]
    fn square_frame_uses_bounded_square_footprint() {
        let (width, height) = media_frame_size(Some((1080, 1080)), CardBand::Wide);
        assert_eq!(width, 480.0);
        assert_eq!(height, 480.0);
        assert!((width / height - 1.0).abs() < 1e-6);
    }

    #[test]
    fn near_square_preserves_exact_ratio_instead_of_forcing_perfect_square() {
        // 1080x1200 (ratio 0.9) is near-square but slightly tall: the frame
        // must stay ratio-exact (0.9), NOT be forced to a perfect square.
        // The height cap (520) wins, so the width derives from the cap to
        // preserve 0.9 exactly.
        let (width, height) = media_frame_size(Some((1080, 1200)), CardBand::Wide);
        assert_eq!(height, 520.0);
        assert!(
            (width - 468.0).abs() < 0.01,
            "width {width} should derive to preserve ratio"
        );
        assert!((width / height - 1080.0 / 1200.0).abs() < 1e-6);

        // 1200x1080 (ratio 1.111) is near-square but slightly wide: the
        // preferred width cap (480) wins and the height derives to keep the
        // exact ratio — again no forced perfect square.
        let (width2, height2) = media_frame_size(Some((1200, 1080)), CardBand::Wide);
        assert_eq!(width2, 480.0);
        assert!((width2 / height2 - 1200.0 / 1080.0).abs() < 1e-6);
    }

    #[test]
    fn square_frame_preferred_width_stays_in_spec_band() {
        // VIDCARD-07 spec: preferred width 420-560 px for square videos.
        // A perfect 1:1 uses the class preferred width directly.
        let (width, _height) = media_frame_size(Some((1080, 1080)), CardBand::Wide);
        assert!(
            (420.0..=560.0).contains(&width),
            "square preferred width {width} must stay in the 420-560 px band"
        );
    }

    #[test]
    fn square_frame_max_height_is_bounded() {
        // VIDCARD-07 spec: maximum height ~520 px. Near-square frames that
        // hit the height cap must still keep the exact ratio.
        let (width, height) = media_frame_size(Some((1080, 1200)), CardBand::Wide);
        assert!(height <= 520.0 + 1e-6);
        assert!((width / height - 0.9).abs() < 1e-6);
    }

    #[test]
    fn square_media_frame_is_centred_and_width_capped_not_stretched() {
        // VIDCARD-07: the square preview must feel intentionally centred,
        // not like a landscape frame containing a small square on the left.
        // The media element is wrapped in a Fill-width container that centres
        // it (`center_x(Fill)`), and the frame itself is width-capped via
        // the bounded `MediaFrameSizing` (Fixed size, never plain Fill) — it
        // never stretches to the full card width.
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();

        // The body column wraps the media in a centring container. Anchor on
        // the outer card container so the extraction survives width changes.
        let body = prod
            .split("let mut body = Column::new()")
            .nth(1)
            .and_then(|s| {
                s.split(".style(|t| crate::design_tokens::card_style(t))")
                    .next()
            })
            .expect("card body column block must exist");
        assert!(
            body.contains("container(media).width(Length::Fill).center_x(Length::Fill)"),
            "square media frame must be centred via a Fill wrapper + center_x(Fill)"
        );

        // The media frame itself is width-capped (never plain Fill): the
        // bounded sizing strategy (Task 15) computes a concrete Fixed size
        // from the band caps and the measured chat width, so a square
        // preview stays centred at its capped size and never stretches to
        // the full card width, while shrinking proportionally at narrow
        // window sizes.
        let media_frame = prod
            .split("let preview: iced::Element<'a, AppMessage> = {")
            .nth(1)
            .and_then(|s| s.split("fn status_metadata").next())
            .expect("media frame container block must exist");
        assert!(
            media_frame.contains("sizing.width()"),
            "media frame width must come from the bounded sizing strategy"
        );
        assert!(
            !media_frame.contains("sizing.max_width()"),
            "media frame must not rely on the removed max_width cap (sizing is now concrete)"
        );
        assert!(
            media_frame.contains(".height(sizing.height())"),
            "media frame height must come from the bounded sizing strategy"
        );

        // The sizing itself resolves to Fixed lengths (bounded box), so the
        // frame cannot stretch to the full card width.
        let sizing_src = prod
            .split("struct MediaFrameSizing")
            .nth(1)
            .and_then(|s| s.split("/// Neutral dark media background").next())
            .expect("MediaFrameSizing impl must exist");
        assert!(
            sizing_src.contains("Length::Fixed(self.width)"),
            "MediaFrameSizing::width must return a Fixed length (bounded, never Fill)"
        );

        // Metadata and actions stay as full-width siblings of the media
        // wrapper in the body column, not inside the capped frame.
        assert!(
            body.contains(".push(status)") && body.contains(".push(actions)"),
            "status and actions must remain full-width card sections outside the media frame"
        );
    }

    #[test]
    fn portrait_frame_caps_height_and_preserves_ratio() {
        // 9:16 is height-capped; the width derives to preserve 0.5625 exactly.
        let (width, height) = media_frame_size(Some((720, 1280)), CardBand::Wide);
        assert_eq!(height, 520.0);
        assert!((width - 292.5).abs() < 0.01);
        assert!((width / height - 720.0 / 1280.0).abs() < 1e-6);
    }

    #[test]
    fn portrait_frame_satisfies_task8_bounds() {
        // VIDCARD-08 Task 8: portrait frames must be narrow (preferred
        // 280-380px, never wider than min(100%, 420px)), height-capped
        // (~520-600px), and always preserve the exact source ratio.
        for (width, height) in [(720u32, 1280u32), (1080, 1920), (576, 1024), (480, 640)] {
            let ratio = width as f32 / height as f32;
            assert!(ratio < 0.85, "fixture must be portrait");
            let (frame_w, frame_h) = media_frame_size(Some((width, height)), CardBand::Wide);
            assert!(
                frame_w <= 420.0,
                "portrait width must never exceed min(100%, 420px), got {frame_w}"
            );
            assert!(
                frame_h <= 600.0,
                "portrait height must stay within the ~520-600px cap, got {frame_h}"
            );
            assert!(
                (frame_w / frame_h - ratio).abs() < 1e-4,
                "frame must preserve the exact source ratio"
            );
        }
        // 9:16 lands in the preferred 280-380px band with the height cap
        // applied, and the responsive max width is exactly the nominal width.
        let (frame_w, frame_h) = media_frame_size(Some((720, 1280)), CardBand::Wide);
        assert!(
            (280.0..=380.0).contains(&frame_w),
            "9:16 width {frame_w} outside the preferred 280-380px band"
        );
        assert!(
            (520.0..=600.0).contains(&frame_h),
            "9:16 height {frame_h} outside the ~520-600px height cap band"
        );
    }

    #[test]
    fn media_frame_sizing_bounds_to_available_width() {
        // Task 15: the frame starts from the measured chat width (capped by
        // the band's nominal cap) and derives the height ratio-exact, so the
        // preview shrinks proportionally at narrow window sizes instead of
        // overflowing, while a portrait never becomes unreasonably tall. The
        // result is a concrete Fixed box shared by poster and player.
        let sizing = MediaFrameSizing::new(Some((720, 1280)), CardBand::Wide, 1000.0);
        assert_eq!(sizing.width, 292.5);
        assert_eq!(sizing.height, 520.0);
        assert_eq!(sizing.width(), Length::Fixed(292.5));
        assert_eq!(sizing.height(), Length::Fixed(520.0));
    }

    #[test]
    fn media_frame_sizing_stays_bounded_without_thumbnail_or_dims() {
        // Unknown dimensions (spec's safe 16:9 default while metadata loads)
        // → bounded default frame, never a frame with no size driver. The
        // fallback tracks the landscape width bound (VIDCARD-06: 720×405).
        let unknown = MediaFrameSizing::new(None, CardBand::Wide, 1000.0);
        assert_eq!(unknown.width(), Length::Fixed(720.0));
        assert_eq!(unknown.height(), Length::Fixed(405.0));

        // Known dimensions but no thumbnail yet (poster still generating) →
        // the frame stays bounded at the nominal size while the poster loads.
        let no_thumb = MediaFrameSizing::new(Some((720, 1280)), CardBand::Wide, 1000.0);
        assert_eq!(no_thumb.width(), Length::Fixed(292.5));
        assert_eq!(no_thumb.height(), Length::Fixed(520.0));
    }

    #[test]
    fn media_frame_sizing_nominal_matches_media_frame_size() {
        // With a chat column wide enough that the cap wins, the sizing
        // matches the nominal bounded box exactly.
        let sizing = MediaFrameSizing::new(Some((3840, 2160)), CardBand::Wide, 1000.0);
        let (width, height) = media_frame_size(Some((3840, 2160)), CardBand::Wide);
        assert_eq!(sizing.width, width);
        assert_eq!(sizing.height, height);
    }

    #[test]
    fn media_frame_sizing_shrinks_to_available_width() {
        // A landscape frame in a 400 px chat column: never wider than the
        // available space (Task 15 no-horizontal-scroll rule) and still
        // ratio-exact.
        let sizing = MediaFrameSizing::new(Some((1280, 720)), CardBand::Wide, 400.0);
        assert_eq!(sizing.width, 400.0);
        assert!((sizing.height - 225.0).abs() < 0.01);
        assert!((sizing.width / sizing.height - 1280.0 / 720.0).abs() < 1e-6);
    }

    #[test]
    fn media_frame_uses_neutral_dark_background_and_bounded_sizing() {
        // VIDCARD-08 Task 8 + Task 15: portrait previews sit on a fixed
        // neutral dark media background (letterboxing reads as deliberate)
        // and the frame is bounded by the measured chat width — no
        // full-card stretch, no top/bottom crop, no horizontal overflow at
        // narrow window sizes.
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let frame = prod
            .split("fn media_frame(")
            .nth(1)
            .and_then(|s| s.split("fn status_metadata").next())
            .expect("media_frame body must exist");
        assert!(
            frame.contains("media_frame_style"),
            "media frame must use the shared media-frame style with the neutral-dark background"
        );
        // The shared media-frame style itself must paint the fixed dark
        // neutral (not the theme-aware card surface color).
        let style_fn = prod
            .split("fn media_frame_style(")
            .nth(1)
            .and_then(|s| s.split("#[cfg(feature = \"video-playback\")]").next())
            .expect("media_frame_style body must exist");
        assert!(
            style_fn.contains("MEDIA_FRAME_BACKGROUND"),
            "media_frame_style must use the fixed neutral-dark background"
        );
        assert!(
            !frame.contains("bg_surface("),
            "media frame must not reuse the card surface color (light in light theme)"
        );
        assert!(
            frame.contains("sizing.width()"),
            "media frame must use the bounded sizing width (min(available, cap))"
        );
        assert!(
            frame.contains("ContentFit::Contain"),
            "poster/player must render contain-style (never stretch or crop)"
        );
        assert!(
            frame.contains("Length::Fill"),
            "poster/player must fill the bounded Fixed frame (contain letterboxes inside it)"
        );
    }

    #[test]
    fn ultrawide_frame_stays_bounded_and_ratio_exact() {
        // 21:9 uses the full landscape width; the height follows the exact
        // ratio instead of forcing a 16:9 box — a short, wide frame with no
        // excessive vertical height and nothing cropped.
        let (width, height) = media_frame_size(Some((6720, 2880)), CardBand::Wide);
        assert_eq!(width, 720.0);
        assert!((height - 308.571).abs() < 0.01);
        assert!((width / height - 6720.0 / 2880.0).abs() < 1e-6);
    }

    #[test]
    fn ultrawide_panorama_stays_short_and_ratio_exact() {
        // 32:9 panorama: still the full landscape width, very short frame —
        // the contain rule keeps every pixel visible (no side cropping).
        let (width, height) = media_frame_size(Some((7680, 2160)), CardBand::Wide);
        assert_eq!(width, 720.0);
        assert!((height - 202.5).abs() < 0.01);
        assert!((width / height - 7680.0 / 2160.0).abs() < 1e-6);
    }

    #[test]
    fn task19_aspect_ratio_matrix_verifies_all_spec_dimensions() {
        // VIDCARD-19 acceptance: run the spec's Task 19 matrix — every one
        // of the ten source-dimension cases must produce a ratio-exact,
        // bounded media frame with no stretching, squashing, unintended
        // cropping, excessive card height, or horizontal overflow, and the
        // frame must stay ratio-exact and bounded at narrow chat widths
        // (the same frame drives poster AND player, Task 10).
        let cases: &[(&str, Option<(u32, u32)>, f32, f32)] = &[
            // (case label, source dimensions, expected wide width, expected wide height)
            ("standard landscape 1920x1080", Some((1920, 1080)), 720.0, 405.0),
            ("hd landscape 1280x720", Some((1280, 720)), 720.0, 405.0),
            ("ultrawide 2560x1080", Some((2560, 1080)), 720.0, 303.75),
            ("classic landscape 640x480", Some((640, 480)), 666.6667, 500.0),
            ("square 1080x1080", Some((1080, 1080)), 480.0, 480.0),
            ("near-square 1080x1200", Some((1080, 1200)), 468.0, 520.0),
            ("vertical 1080x1920", Some((1080, 1920)), 292.5, 520.0),
            ("tall vertical 720x1600", Some((720, 1600)), 234.0, 520.0),
            ("small landscape 320x180", Some((320, 180)), 720.0, 405.0),
            ("unknown metadata (no dims)", None, 720.0, 405.0),
        ];

        for (label, dims, expected_w, expected_h) in cases {
            let ratio = intrinsic_ratio(*dims);

            // Wide band: the frame is exactly ratio-exact (no stretch /
            // squash / crop) and bounded by the class caps.
            let (width, height) = media_frame_size(*dims, CardBand::Wide);
            assert!(
                (width - expected_w).abs() < 0.01,
                "{label}: wide width {width} != expected {expected_w}"
            );
            assert!(
                (height - expected_h).abs() < 0.01,
                "{label}: wide height {height} != expected {expected_h}"
            );
            assert!(
                (width / height - ratio).abs() < 1e-4,
                "{label}: wide frame ratio {} != intrinsic {ratio}",
                width / height
            );
            // No excessive card height / horizontal overflow at wide.
            assert!(
                height <= 520.0 + 1e-6,
                "{label}: wide height {height} exceeds the 520 px cap"
            );
            assert!(
                width <= 720.0 + 1e-6,
                "{label}: wide width {width} exceeds the 720 px cap"
            );

            // Narrow chat column (Task 15): the same case must shrink to fit
            // the available width, stay ratio-exact, and stay bounded — no
            // horizontal overflow, no excessive card height.
            let narrow = MediaFrameSizing::new(*dims, CardBand::Narrow, 352.0);
            assert!(
                narrow.width <= 352.0 + 1e-6,
                "{label}: narrow width {} overflows a 352 px column",
                narrow.width
            );
            assert!(
                (narrow.width / narrow.height - ratio).abs() < 1e-3,
                "{label}: narrow frame ratio {} != intrinsic {ratio}",
                narrow.width / narrow.height
            );
            let narrow_height_cap = 520.0 * CardBand::Narrow.media_scale();
            assert!(
                narrow.height <= narrow_height_cap + 1e-6,
                "{label}: narrow height {} exceeds the narrow cap {narrow_height_cap}",
                narrow.height
            );

            // Poster and player share this exact frame (Task 10): the sizing
            // is computed once from the same dimensions/band and both the
            // poster stack and the player stack use `.width(sizing.width())`
            // + `.height(sizing.height())` — pinned by the structural test
            // `media_frame_keeps_poster_and_player_geometry_identical`.
            assert_eq!(
                MediaFrameSizing::new(*dims, CardBand::Narrow, 352.0),
                narrow,
                "{label}: sizing must be deterministic (poster == player frame)"
            );
        }
    }

    #[test]
    fn controls_overlay_cannot_change_media_frame_geometry() {
        // The controls are deliberately a full-frame stack layer.  Keep this
        // structural guard close to the ratio matrix: changing the overlay to
        // an intrinsic-height sibling would reintroduce layout shift when the
        // auto-hide state changes.
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let player = prod
            .split("let controls_overlay: iced::Element<'a, AppMessage>")
            .nth(1)
            .and_then(|s| s.split(".style(media_frame_style)").next())
            .expect("controls overlay block must exist");
        assert!(
            player.contains(".width(Length::Fill)")
                && player.contains(".height(Length::Fill)"),
            "visible and hidden controls must occupy the same full-frame layer"
        );
        let frame = prod
            .split("container(widget::stack![\n                video_element,")
            .nth(1)
            .and_then(|s| s.split(".into()\n        } else").next())
            .expect("player stack must exist");
        assert!(
            frame.contains(".width(sizing.width())")
                && frame.contains(".height(sizing.height())"),
            "player stack must use the same fixed sizing as the poster"
        );
    }

    #[test]
    fn media_container_meets_task16_subtle_surface_contract() {
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        assert!(prod.contains("const MEDIA_FRAME_RADIUS: f32 = 13.0"));
        assert!(prod.contains("const MEDIA_FRAME_BACKGROUND: Color = Color::from_rgb"));
        assert!(prod.contains(".clip(true)"));
        assert!(
            !prod.contains("shadow:") || prod.matches("shadow:").count() == 0,
            "the inline media container must not add a heavy shadow"
        );
    }

    #[test]
    fn video_state_mapping_requires_verified_local_path() {
        let mut attachment =
            DownloadAttachment::new(TransferKind::Video, "clip.mp4", "ticket", "peer", None);
        assert_eq!(
            video_presentation_state(&attachment),
            VideoPresentationState::Remote
        );
        attachment.state = DownloadState::Active {
            bytes: 10,
            total: Some(100),
        };
        assert_eq!(
            video_presentation_state(&attachment),
            VideoPresentationState::Downloading
        );
        attachment.state = DownloadState::Completed {
            saved_name: "clip.mp4".into(),
            saved_path: None,
            total_size: Some(100),
        };
        assert_eq!(
            video_presentation_state(&attachment),
            VideoPresentationState::Verifying
        );
    }

    #[test]
    fn video_state_mapping_recovers_from_missing_local_file() {
        let mut attachment =
            DownloadAttachment::new(TransferKind::Video, "clip.mp4", "ticket", "peer", None);
        attachment.state = DownloadState::Completed {
            saved_name: "clip.mp4".into(),
            saved_path: Some(PathBuf::from("/definitely/missing/clip.mp4")),
            total_size: Some(100),
        };
        assert_eq!(
            video_presentation_state(&attachment),
            VideoPresentationState::Missing
        );
        attachment.state = DownloadState::Failed {
            failure: DownloadFailure::FileRemoved,
        };
        assert_eq!(
            video_presentation_state(&attachment),
            VideoPresentationState::Missing
        );
    }

    #[test]
    fn format_label_uses_real_extension_case_insensitively() {
        assert_eq!(file_format_label("clip.mp4"), Some("MP4".to_string()));
        assert_eq!(
            file_format_label("summer-trip.MOV"),
            Some("MOV".to_string())
        );
        assert_eq!(file_format_label("no_extension"), None);
    }

    #[test]
    fn placeholder_shows_loading_while_metadata_or_thumbnail_is_pending() {
        let mut attachment =
            DownloadAttachment::new(TransferKind::Video, "clip.mp4", "ticket", "peer", None);
        // Remote/not-yet-downloaded videos keep the stable default copy.
        assert_eq!(
            media_placeholder_text(&attachment),
            "Preview available after download"
        );
        attachment.metadata_loading = true;
        assert_eq!(media_placeholder_text(&attachment), "Loading preview…");
        // Sender published a poster blob → the fetch is pending.
        attachment.metadata_loading = false;
        attachment.thumbnail_hash = Some([0xab; 32]);
        assert_eq!(media_placeholder_text(&attachment), "Loading preview…");
        // Once the handle arrives the placeholder is no longer used.
        attachment.thumbnail_hash = None;
        attachment.thumbnail_handle = Some(iced::widget::image::Handle::from_bytes(vec![1, 2, 3]));
        assert_eq!(
            media_placeholder_text(&attachment),
            "Preview available after download"
        );
    }

    #[test]
    fn placeholder_falls_back_to_bounded_generic_frame_on_probe_failure() {
        let mut attachment =
            DownloadAttachment::new(TransferKind::Video, "clip.mp4", "ticket", "peer", None);
        attachment.metadata_loading = true;
        attachment.metadata_failed = true;
        // A failed probe never leaves the user with a growing placeholder:
        // the frame stays bounded (16:9 default) and the copy is explicit.
        assert_eq!(media_placeholder_text(&attachment), "Preview unavailable");
        let (width, height) = media_frame_size(None, CardBand::Wide);
        assert_eq!(width, 720.0);
        assert!((height - 405.0).abs() < 0.01);
    }

    #[test]
    fn card_source_wires_loading_placeholder_into_media_frame() {
        // VIDCARD-09 acceptance: the media frame must render a stable loading
        // placeholder while metadata loads, then swap to the ratio-exact frame
        // without a large layout jump (the frame is always bounded).
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        assert!(
            prod.contains("media_placeholder_text(attachment)"),
            "media frame must use the loading/unavailable placeholder helper"
        );
        assert!(
            prod.contains("metadata_loading"),
            "card must track the async metadata-load state"
        );
        // The placeholder and the final media share the same bounded frame
        // sizing helper (`MediaFrameSizing` derives its fixed nominal box from
        // `media_frame_size`, VIDCARD-08), so replacing the placeholder never
        // causes a large layout jump.
        assert!(
            prod.contains("MediaFrameSizing::new(\n            attachment.poster_dimensions"),
            "media frame must derive sizing from the attachment's dimensions"
        );
        assert!(
            prod.contains("fn media_frame_size"),
            "bounded frame sizing helper must exist"
        );
    }

    #[test]
    fn card_surface_uses_the_modern_boru_card_style() {
        // VIDCARD-03: the card surface must reuse the shared design-system
        // card style (soft white theme-aware surface, thin green-grey
        // border, RADIUS_CARD 16 px, very subtle shadow) with 20-24 px
        // internal padding and shared-scale section spacing. The outer
        // card must never hide layout defects with clipping — `.clip(true)`
        // is only allowed at the media-frame boundary (spec Task 11).
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();

        // Inspect only the outer card container block (between the body
        // column and its terminating `.into()`).
        let outer = prod
            .split("container(body)")
            .nth(1)
            .and_then(|s| s.split(".into()").next())
            .expect("outer card container block must exist");
        assert!(
            outer.contains("crate::design_tokens::card_style"),
            "card surface must reuse design_tokens::card_style"
        );
        assert!(
            outer.contains(".padding([SPACE_20, SPACE_24])"),
            "card padding must use the 20-24 px token band"
        );
        assert!(
            outer.contains(".width(outer_width)"),
            "card width must be responsive (content-driven Shrink at wide/medium, Fill at narrow)"
        );
        assert!(
            !outer.contains(".clip("),
            "the outer card surface must not rely on hidden overflow"
        );

        // Consistent shared-scale spacing between the card sections.
        let section_gap_count = prod.matches(".spacing(SPACE_12)").count();
        assert!(
            section_gap_count >= 2,
            "card section gaps must use shared-scale SPACE_12, got {section_gap_count}"
        );
    }

    #[test]
    fn truncate_filename_keeps_short_names_untouched() {
        assert_eq!(truncate_filename("clip.mp4", 56), "clip.mp4");
        assert_eq!(truncate_filename("", 56), "");
    }

    #[test]
    fn truncate_filename_keeps_extension_visible() {
        let long = format!("{}.mp4", "a".repeat(120));
        let out = truncate_filename(&long, HEADER_FILENAME_MAX_CHARS);
        assert!(out.ends_with(".mp4"), "extension dropped: {out}");
        assert!(out.chars().count() <= HEADER_FILENAME_MAX_CHARS);
        assert!(out.contains('…'));
    }

    #[test]
    fn truncate_filename_without_extension_uses_tail_ellipsis() {
        let long = "b".repeat(120);
        let out = truncate_filename(&long, HEADER_FILENAME_MAX_CHARS);
        assert_eq!(out.chars().count(), HEADER_FILENAME_MAX_CHARS);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_filename_respects_char_boundaries() {
        // Multi-byte characters must never be split mid-codepoint.
        let long = format!("{}.mp4", "视频".repeat(60));
        let out = truncate_filename(&long, HEADER_FILENAME_MAX_CHARS);
        assert!(out.ends_with(".mp4"));
        assert!(out.chars().count() <= HEADER_FILENAME_MAX_CHARS);
    }

    #[test]
    fn header_badge_uses_only_real_states() {
        let theme = iced::Theme::Light;
        let mut attachment =
            DownloadAttachment::new(TransferKind::Video, "clip.mp4", "ticket", "peer", None);

        attachment.state = DownloadState::Active {
            bytes: 10,
            total: Some(100),
        };
        assert_eq!(header_badge(&attachment.state, &theme).0, "Downloading");

        attachment.state = DownloadState::Completed {
            saved_name: "clip.mp4".into(),
            saved_path: None,
            total_size: Some(100),
        };
        assert_eq!(header_badge(&attachment.state, &theme).0, "Downloaded");

        attachment.state = DownloadState::Completed {
            saved_name: "clip.mp4".into(),
            saved_path: Some(PathBuf::from("/definitely/missing/clip.mp4")),
            total_size: Some(100),
        };
        assert_eq!(header_badge(&attachment.state, &theme).0, "Unavailable");

        attachment.state = DownloadState::Failed {
            failure: DownloadFailure::Other {
                detail: "boom".into(),
            },
        };
        assert_eq!(header_badge(&attachment.state, &theme).0, "Failed");
    }

    #[test]
    fn media_frame_uses_spec_radius_dark_surface_and_boundary_clip() {
        // VIDCARD-11: the media frame must use a ~12–14 px radius, a
        // neutral dark background, a thin subtle border, and hidden overflow
        // ONLY at the media-frame boundary (never on the outer card).
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();

        assert!(
            prod.contains("const MEDIA_FRAME_RADIUS: f32 = 13.0;"),
            "media frame radius must sit in the 12–14 px spec band"
        );
        assert!(
            prod.contains("const MEDIA_FRAME_BACKGROUND: Color"),
            "media frame must define a neutral dark background"
        );
        assert!(
            prod.contains("const MEDIA_FRAME_BORDER: Color"),
            "media frame must define a thin subtle border"
        );
        // The shared surface style is applied to both the poster frame and
        // the player frame; the media frame is the ONLY boundary that clips.
        let media_frame_fns = prod
            .split("fn media_frame(")
            .nth(1)
            .expect("media_frame must exist");
        assert!(
            media_frame_fns.contains(".clip(true)"),
            "media frame must clip overflow at its own boundary"
        );
        // The outer card surface must not rely on hidden overflow.
        let outer = prod
            .split("container(body)")
            .nth(1)
            .and_then(|s| s.split(".into()").next())
            .expect("outer card container block must exist");
        assert!(
            !outer.contains(".clip("),
            "the outer card surface must not clip (spec Task 11)"
        );
    }

    // ── Task 15: responsive card behaviour ──────────────────────────────

    #[test]
    fn card_band_classifies_timeline_widths() {
        use CardBand::*;
        assert_eq!(CardBand::of(320.0), Narrow);
        assert_eq!(CardBand::of(NARROW_CARD_BREAKPOINT), Narrow);
        assert_eq!(CardBand::of(NARROW_CARD_BREAKPOINT + 1.0), Medium);
        assert_eq!(CardBand::of(MEDIUM_CARD_BREAKPOINT - 1.0), Medium);
        assert_eq!(CardBand::of(MEDIUM_CARD_BREAKPOINT), Wide);
        assert_eq!(CardBand::of(1280.0), Wide);
    }

    #[test]
    fn media_caps_reduce_at_medium_and_narrow_bands() {
        // Task 15: "reduce media maximum dimensions" at medium widths. A
        // 1280×720 landscape preview keeps the full 720×405 box at Wide and
        // shrinks proportionally at Medium (×0.85) and Narrow (×0.7).
        let (wide_w, wide_h) = media_frame_size(Some((1280, 720)), CardBand::Wide);
        assert_eq!(wide_w, 720.0);
        assert!((wide_h - 405.0).abs() < 0.01);

        let (medium_w, medium_h) = media_frame_size(Some((1280, 720)), CardBand::Medium);
        assert!((medium_w - 720.0 * 0.85).abs() < 0.01);
        assert!((medium_w / medium_h - 1280.0 / 720.0).abs() < 1e-6);

        let (narrow_w, narrow_h) = media_frame_size(Some((1280, 720)), CardBand::Narrow);
        assert!((narrow_w - 720.0 * 0.7).abs() < 0.01);
        assert!((narrow_w / narrow_h - 1280.0 / 720.0).abs() < 1e-6);
    }

    #[test]
    fn portrait_frame_never_exceeds_height_cap_at_narrow_widths() {
        // Task 15: "Do NOT switch a portrait preview to full-width merely
        // because the window becomes narrower if that would make it
        // unreasonably tall." A 9:16 portrait inside a 352 px chat column
        // stays height-capped and narrow instead of stretching to the column.
        let sizing = MediaFrameSizing::new(Some((1080, 1920)), CardBand::Narrow, 352.0);
        // Narrow portrait caps: 380*0.7 wide, 520*0.7 tall.
        let max_height = 520.0 * CardBand::Narrow.media_scale();
        assert!(
            sizing.height <= max_height + 1e-6,
            "portrait height {} exceeds the narrow cap {max_height}",
            sizing.height
        );
        assert!(
            (sizing.width / sizing.height - 1080.0 / 1920.0).abs() < 1e-4,
            "portrait frame must stay ratio-exact"
        );
        assert!(
            sizing.width <= 352.0,
            "portrait frame must never exceed the available chat width"
        );
    }

    #[test]
    fn square_frame_stays_centred_and_bounded_at_narrow_widths() {
        // A square preview at narrow widths fits the column exactly without
        // stretching to the full card width.
        let sizing = MediaFrameSizing::new(Some((1080, 1080)), CardBand::Narrow, 352.0);
        assert!(sizing.width <= 352.0);
        assert!((sizing.width - sizing.height).abs() < 1e-6);
        assert!((sizing.width / sizing.height - 1.0).abs() < 1e-6);
    }

    #[test]
    fn card_outer_width_fills_at_narrow_band() {
        // Task 15: "At narrow widths: Card width becomes 100%". The outer
        // card container picks Fill at the Narrow band and stays
        // content-driven (Shrink) at wide/medium so a portrait card never
        // spans the whole chat width.
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let outer = prod
            .split("let outer_width = if self.band() == CardBand::Narrow")
            .nth(1)
            .and_then(|s| {
                s.split(".style(|t| crate::design_tokens::card_style(t))")
                    .next()
            })
            .expect("outer card width block must exist");
        assert!(
            outer.contains("Length::Fill"),
            "Narrow band must set the card width to Fill (100% of the chat column)"
        );
        assert!(
            outer.contains("Length::Shrink"),
            "wide/medium bands must keep the card content-driven (Shrink)"
        );
        assert!(
            outer.contains(".width(outer_width)"),
            "the outer card container must apply the responsive width"
        );
    }

    #[test]
    fn play_overlay_is_circular_high_contrast_and_has_accessible_label() {
        // VIDCARD-11: the play overlay must be a centred, circular,
        // semi-transparent dark button with a strong-contrast glyph, a
        // keyboard-accessible button widget, and an accessible label such
        // as "Play video".
        //
        // VIDCARD-17: iced 0.14 buttons have no `operation::Focusable` impl
        // and no keyboard handling, so keyboard accessibility is delivered
        // by wrapping the overlay in
        // `crate::focusable_button::focusable_button`, which joins the Tab
        // traversal, activates on Enter/Space and draws a visible focus ring.
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let media_frame_fns = prod
            .split("fn media_frame(")
            .nth(1)
            .expect("media_frame must exist");

        assert!(
            media_frame_fns.contains("Icon::Play"),
            "play overlay must use the play icon"
        );
        assert!(
            media_frame_fns.contains("MEDIA_FRAME_OVERLAY_BG"),
            "play overlay must use the semi-transparent dark surface"
        );
        assert!(
            media_frame_fns.contains("PLAY_OVERLAY_SIZE"),
            "play overlay must be sized by the restrained-size constant"
        );
        assert!(
            media_frame_fns.contains("\"Play video\""),
            "play overlay must expose an accessible 'Play video' label"
        );
        assert!(
            media_frame_fns.contains("button("),
            "play overlay must be a real button"
        );
        assert!(
            media_frame_fns.contains("focusable_button::focusable_button("),
            "play overlay must be wrapped in the focusable button for keyboard access"
        );
        assert!(
            media_frame_fns.contains(".ring_radius(PLAY_OVERLAY_SIZE / 2.0)"),
            "play overlay focus ring must follow the circular button radius"
        );
        assert!(
            media_frame_fns.contains("container(play)")
                && media_frame_fns.contains(".center_x(Length::Fill)")
                && media_frame_fns.contains(".center_y(Length::Fill)"),
            "play overlay must be centred inside the media frame (Task 19 matrix)"
        );
    }

    #[test]
    fn play_action_routes_to_inline_player_for_ready_videos() {
        // VID-03: clicking Play on a ready video must dispatch the inline
        // player message — never silently fall back to opening the OS
        // player. The feature-gated inline route is the primary path; the
        // OS-open fallback must be explicitly confined to the non-feature
        // build (and only reachable for non-ready states).
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let media_frame_fns = prod
            .split("fn media_frame(")
            .nth(1)
            .expect("media_frame must exist");

        // The inline route is chosen whenever the `video-playback` feature
        // is compiled in.
        let play_message_block = media_frame_fns
            .split("let play_message = {")
            .nth(1)
            .and_then(|s| s.split("};").next())
            .expect("play_message block must exist");
        assert!(
            play_message_block.contains("#[cfg(feature = \"video-playback\")]")
                && play_message_block.contains("AppMessage::PlayInlineVideo(self.entry_index)"),
            "with video-playback enabled the play overlay must dispatch PlayInlineVideo"
        );
        // The OS-open fallback exists only under the explicit non-feature
        // cfg — it must not be the default route.
        assert!(
            play_message_block.contains("#[cfg(not(feature = \"video-playback\"))]")
                && play_message_block.contains("AppMessage::OpenDownloadedFile(attachment.name.clone())"),
            "OS-open fallback must be confined to the non-feature build"
        );

        // The play overlay is only enabled for Ready videos (and not while
        // the player is still preparing), so an external player can never be
        // spawned for a video that is still downloading/verifying.
        assert!(
            media_frame_fns.contains("presentation == VideoPresentationState::Ready && !self.preparing"),
            "play overlay must be enabled only when the video is Ready and not preparing"
        );
        assert!(
            media_frame_fns.contains(".on_press_maybe(play_enabled.then_some(play_message.clone()))"),
            "play overlay must dispatch the play message only when enabled"
        );
    }

    #[test]
    fn header_filename_is_flexible_at_narrow_band() {
        // Task 15: "Filenames truncate safely" at narrow widths — the
        // filename fills the space left by the other header items (still
        // capped and clipped) instead of forcing the card wider.
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let header = prod
            .split("fn header(")
            .nth(1)
            .and_then(|s| s.split("fn overflow_menu").next())
            .expect("header body must exist");
        assert!(
            header.contains(".width(if narrow { Length::Fill } else { Length::Shrink })"),
            "header filename must be flexible at narrow widths and content-driven otherwise"
        );
        assert!(
            header.contains("title_row = title_row.width(Length::Fill)"),
            "header title row must fill the card at narrow widths"
        );
        assert!(
            header.contains(".max_width(HEADER_FILENAME_MAX_WIDTH)"),
            "header filename must stay capped"
        );
        assert!(
            header.contains(".clip(true)"),
            "header filename must clip so long names truncate safely"
        );
    }

    #[test]
    fn player_has_one_compact_timing_indicator() {
        // PDF task 6: the duplicate lower-right duration badge was removed;
        // timing is rendered once in the main control row from live metadata.
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let media_frame_fns = prod
            .split("fn media_frame(")
            .nth(1)
            .expect("media_frame must exist");

        assert!(
            media_frame_fns.contains("format_media_time(position)")
                && media_frame_fns.contains("format_media_time(duration)"),
            "timing must use live position and duration metadata"
        );
        assert!(
            !media_frame_fns.contains("duration_badge")
                && !media_frame_fns.contains("DURATION_BADGE_ZONE"),
            "the duplicate duration badge must not be rendered"
        );
    }

    #[test]
    fn loading_indicator_present_while_poster_or_player_prepares() {
        // VIDCARD-11: a loading indicator must exist while the poster
        // (downloading/verifying) or the inline player (preparing) is being
        // prepared.
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let media_frame_fns = prod
            .split("fn media_frame(")
            .nth(1)
            .expect("media_frame must exist");

        assert!(
            prod.contains("fn loading_indicator"),
            "a loading indicator must be defined"
        );
        assert!(
            media_frame_fns.contains("self.preparing"),
            "player preparation must surface the loading indicator"
        );
        assert!(
            media_frame_fns.contains("VideoPresentationState::Downloading")
                && media_frame_fns.contains("VideoPresentationState::Verifying"),
            "poster preparation (downloading/verifying) must surface the loading indicator"
        );
    }

    #[test]
    fn media_frame_keeps_poster_and_player_geometry_identical() {
        // Task 10 invariant: the poster and the player must share the same
        // media box so Play does not cause a layout jump. VIDCARD-11 must
        // preserve that on top of VIDCARD-15's responsive sizing: both the
        // poster frame and the player frame are driven by the same concrete
        // MediaFrameSizing (sizing) and share the same media-frame surface
        // style and boundary clip. The player replaces the poster INSIDE
        // that same frame — the frame is not duplicated, the card is not
        // rebuilt, and the controls overlay the frame bottom rather than
        // adding a new row below it.
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let media_frame_fns = prod
            .split("fn media_frame(")
            .nth(1)
            .expect("media_frame must exist");

        // One shared sizing instance sizes BOTH frames: the poster stack
        // (`container(widget::stack![poster, ...])`) and the player stack
        // (`container(widget::stack![video_element, ...])`) both call
        // `.width(sizing.width()).height(sizing.height())`.
        assert!(
            media_frame_fns.contains("MediaFrameSizing::new("),
            "poster frame must be sized by the shared MediaFrameSizing"
        );
        assert!(
            media_frame_fns.matches(".width(sizing.width())").count() >= 2,
            "poster and player frames must both be sized by the same width"
        );
        assert!(
            media_frame_fns.matches(".height(sizing.height())").count() >= 2,
            "poster and player frames must both be sized by the same height"
        );
        assert!(
            media_frame_fns.matches(".style(media_frame_style)").count() >= 2,
            "poster and player frames must use the same shared surface style (radius + background + border)"
        );
        assert!(
            media_frame_fns.matches(".clip(true)").count() >= 2,
            "poster and player frames must both clip overflow at the frame boundary"
        );
        assert!(
            media_frame_fns.matches("widget::stack![").count() >= 2,
            "both the poster and the player render as a layered stack inside the single media frame"
        );

        // The player replaces the poster inside the same frame: the
        // VideoPlayer element is present, uses Contain (never stretched or
        // cropped), and its controls bar overlays the frame's bottom edge
        // (`align_y(Alignment::End)`) instead of being pushed below the
        // frame as a new card row.
        assert!(
            media_frame_fns.contains("VideoPlayer::new(&video)"),
            "player must render through VideoPlayer inside the media frame"
        );
        assert!(
            media_frame_fns
                .matches("content_fit(iced::ContentFit::Contain)")
                .count()
                >= 2,
            "poster image and player video must both use Contain inside the shared frame"
        );
        assert!(
            media_frame_fns.contains("container(controls_bar)")
                && media_frame_fns.contains(".align_y(Alignment::End)"),
            "playback controls must overlay the frame bottom, inside the frame"
        );

        // The card body always renders exactly one media element — the
        // shared `media` element returned by media_frame — regardless of
        // playback state. The poster→player swap happens inside media_frame,
        // never by appending a second section to the card, so the surrounding
        // chat cannot jump when Play is pressed.
        let view_fns = prod.split("fn view(").nth(1).expect("view must exist");
        assert_eq!(
            view_fns.matches("container(media)").count(),
            1,
            "card body must render exactly one media element (poster OR player), never both"
        );
    }

    #[test]
    fn action_buttons_wrap_at_narrow_widths() {
        // Task 15: "Action buttons may stack vertically or wrap" at narrow
        // widths. The shared action row is a wrapping row, so the buttons
        // stay on one line at wide/medium and flow onto extra lines when
        // they do not fit — never overflowing the chat column horizontally.
        let src = include_str!("download_progress_view.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        assert!(
            prod.contains("Row::with_children(buttons).spacing(SPACE_8).wrap()"),
            "action buttons must use a wrapping row so they wrap/stack at narrow widths"
        );
    }

    // ── VIDCARD-17: Accessibility (spec Task 17) ───────────────────────

    #[test]
    fn every_action_button_is_keyboard_focusable() {
        // Task 17: buttons must be keyboard accessible. iced 0.14's stock
        // Button has no `operation::Focusable` impl and no keyboard event
        // handling, so every shared action-button helper wraps its button in
        // `focusable_button` (Tab traversal + Enter/Space activation).
        let src = include_str!("download_progress_view.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        for helper in [
            "fn action_button",
            "fn text_button",
            "fn primary_button",
            "fn secondary_button",
            "fn disabled_button",
        ] {
            let body = prod
                .split(helper)
                .nth(1)
                .unwrap_or_else(|| panic!("{helper} must exist"));
            let body = body.split("fn ").next().unwrap();
            assert!(
                body.contains("focusable_button::focusable_button("),
                "{helper} must wrap its button in the focusable button wrapper"
            );
        }
    }

    #[test]
    fn disabled_button_does_not_join_focus_order() {
        // A disabled/loading button has no action, so it must pass
        // `None` to the focusable wrapper and stay out of the Tab order
        // (the wrapper only registers `operation.focusable` when on_press
        // is present).
        let src = include_str!("download_progress_view.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let body = prod
            .split("fn disabled_button")
            .nth(1)
            .expect("disabled_button must exist");
        let body = body.split("fn ").next().unwrap();
        assert!(
            body.contains("None,"),
            "disabled_button must not register an activation message"
        );
    }

    #[test]
    fn icon_only_buttons_carry_accessible_names() {
        // Task 17: "All icon-only buttons have accessible names." iced 0.14
        // exposes no aria API, so the project's icon-button convention is a
        // tooltip label: the play overlay says "Play video" and the header
        // overflow kebab says "More actions".
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        assert!(
            prod.contains("\"Play video\""),
            "play overlay must carry an accessible name"
        );
        assert!(
            prod.contains("\"More actions\""),
            "overflow kebab must carry an accessible name"
        );
    }

    #[test]
    fn progress_value_is_real_text() {
        // Task 17: "Download progress has an accessible value." iced 0.14
        // has no progress-bar aria API, so the value must be real text: the
        // pct label and the bytes/total detail line.
        let src = include_str!("download_progress_view.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let progress = prod
            .split("pub(crate) fn progress_section")
            .nth(1)
            .expect("progress_section must exist");
        assert!(
            progress.contains("format!(\"{pct}%\")"),
            "progress section must render the percentage as real text"
        );
        assert!(
            progress.contains("type_role_text"),
            "progress percentage must be a real text element"
        );
        let detail = prod
            .split("pub(crate) fn active_download_detail")
            .nth(1)
            .expect("active_download_detail must exist");
        assert!(
            detail.contains("human_size") && detail.contains("{pct}%"),
            "detail line must render bytes/total/percent as real text"
        );
    }

    #[test]
    fn filename_full_name_is_exposed_to_assistive_technology() {
        // Task 17: "Filename truncation still exposes the full name to
        // assistive technology." The header truncates visually but the full
        // name travels in the tooltip and in the Copy filename action.
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let header = prod
            .split("fn header(")
            .nth(1)
            .expect("header must exist");
        assert!(
            header.contains("attachment.name.clone()"),
            "filename tooltip must carry the full untruncated name"
        );
        assert!(
            header.contains("truncate_filename("),
            "the visible filename is still visually truncated"
        );
        assert!(
            prod.contains("CopyToClipboard(name.clone())"),
            "copy action must expose the full name to the clipboard"
        );
    }

    #[test]
    fn status_is_not_conveyed_by_colour_alone() {
        // Task 17: "Status is not conveyed by colour alone." Every state has
        // a real status word ("Downloading from Duke", "Paused",
        // "Ready to play", ...) rendered as text; colour is an extra cue.
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let status = prod
            .split("fn status_metadata")
            .nth(1)
            .expect("status_metadata must exist");
        assert!(
            status.contains("format!(\"●  {status}\")"),
            "status line must render the state word as real text"
        );
        assert!(
            status.contains("\"Downloading from {}\"")
                && status.contains("\"Paused\"")
                && status.contains("\"Ready to play\""),
            "active/paused/ready states must all carry a textual label"
        );
    }

    #[test]
    fn error_states_provide_text_not_only_icons() {
        // Task 17: "Error states provide text, not only icons." Both the
        // transfer-failure section and the playback-error panel render the
        // real failure title/message/recovery text.  The transfer-failure
        // block is the shared `failure_block` helper (download_progress_view),
        // rendered by the video card inside its fixed error slot.
        let src = include_str!("download_progress_view.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let err = prod
            .split("pub(crate) fn failure_block")
            .nth(1)
            .expect("failure_block must exist");
        assert!(
            err.contains("failure.title()")
                && err.contains("failure.message()")
                && err.contains("failure.recovery_action()"),
            "transfer failure must render title/message/recovery as text"
        );
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let media = prod
            .split("fn media_frame(")
            .nth(1)
            .expect("media_frame must exist");
        assert!(
            media.contains("error.title()") && media.contains("error.message()"),
            "playback error panel must render title and message as text"
        );
    }

    #[test]
    fn player_control_buttons_use_accessible_icon_controls() {
        // PDF tasks 5/7 and Task 17: media controls use the established icon
        // set but retain accessible tooltip names and focusable wrappers.
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let media = prod
            .split("fn media_frame(")
            .nth(1)
            .expect("media_frame must exist");
        assert!(
            media.contains("Icon::Play") && media.contains("Icon::Pause"),
            "play/pause control must use media icons"
        );
        assert!(
            media.contains("Icon::VolumeX")
                && media.contains("Icon::Volume1")
                && media.contains("Icon::Volume2"),
            "volume control must represent muted, low and high states"
        );
        assert!(
            media.contains("\"Play video\"")
                && media.contains("\"Pause video\"")
                && media.contains("\"Mute\"")
                && media.contains("\"Unmute\""),
            "icon controls must retain accessible names"
        );
        assert!(
            media.contains("media_icon_button(")
                && media.contains("focusable_button::focusable_button("),
            "player controls must use the focusable icon-button helper"
        );
    }

    #[test]
    fn focus_ring_uses_visible_design_token_colors() {
        // Task 17: "Focus indicators are visible." The focusable-button
        // wrapper draws a 2 px ring using the design token focus colour on
        // both light and dark themes (contrast-tested in design_tokens).
        let src = include_str!("focusable_button.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        assert!(
            prod.contains("color_focus(theme)"),
            "focus ring must use the design-token focus colour"
        );
        assert!(
            prod.contains("design_tokens::FOCUS_WIDTH"),
            "focus ring must use the design-token ring width"
        );
        assert!(
            prod.contains("state.is_focused"),
            "focus ring must only draw while the button is focused"
        );
    }

    #[test]
    fn action_button_order_is_logical() {
        // Task 17: "Button order is logical." The action row must present
        // the primary action first, supporting actions second, and the
        // destructive action last (Cancel/Remove are text buttons at the
        // tail of every state's row).
        let src = include_str!("download_progress_view.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let action_buttons = prod
            .split("pub(crate) fn action_buttons")
            .nth(1)
            .expect("action_buttons must exist");
        // In every state the destructive text_button appears after the
        // primary/secondary entries in the vec.
        let completed = action_buttons
            .split("(Video, DownloadState::Completed { saved_path: Some(path), .. }) if path.exists()")
            .nth(1)
            .expect("completed video arm must exist");
        assert!(
            completed.contains("primary_button") && completed.contains("secondary_button"),
            "completed state must lead with primary then secondary actions"
        );
        let active = action_buttons
            .split("(_, DownloadState::Active { .. })")
            .nth(1)
            .expect("active arm must exist");
        assert!(
            active.find("text_button").unwrap_or(usize::MAX)
                > active.find("secondary_button").unwrap_or(usize::MAX),
            "destructive Cancel must come after the secondary Pause button"
        );
        let failed = action_buttons
            .split("(_, DownloadState::Failed { failure }) if failure.retry_available()")
            .nth(1)
            .expect("failed arm must exist");
        assert!(
            failed.find("text_button").unwrap_or(usize::MAX)
                > failed.find("primary_button").unwrap_or(usize::MAX),
            "destructive Remove must come after the primary Retry button"
        );
    }

    #[test]
    fn video_card_has_no_animation_to_reduce() {
        // Task 17: "Reduced-motion preferences are respected for progress
        // and hover animation where supported." The card renders no animated
        // progress and no hover motion: the progress bar is a static fill
        // and the loading indicator is a static icon + label. The app's
        // reduced_motion flag gates the animated spinners elsewhere.
        let card_src = include_str!("video_file_card.rs");
        let card_prod = card_src.split("#[cfg(test)]").next().unwrap();
        let dpv_src = include_str!("download_progress_view.rs");
        let dpv_prod = dpv_src.split("#[cfg(test)]").next().unwrap();
        let progress = dpv_prod
            .split("pub(crate) fn progress_section")
            .nth(1)
            .expect("progress_section must exist");
        assert!(
            !progress.contains("animation") && !progress.contains("Animation"),
            "progress bar must be a static fill (no animation)"
        );
        let loading = card_prod
            .split("fn loading_indicator")
            .nth(1)
            .expect("loading_indicator must exist");
        assert!(
            !loading.contains("animation") && !loading.contains("Animation"),
            "loading indicator must be a static icon + label (no animation)"
        );
    }

    // ── Shape stability (this task) ────────────────────────────────────

    /// Lay out a video card element offscreen (tiny-skia CPU renderer) and
    /// return the outer node bounds.  Same harness as the FONTS-17 captures.
    #[cfg(feature = "video-playback")]
    fn measure_outer_bounds(
        element: &mut iced::Element<'static, AppMessage>,
        canvas: (f32, f32),
    ) -> (f32, f32) {
        use iced::advanced::layout;
        use iced::advanced::widget::Tree;
        use iced::{Font, Pixels, Size};

        let mut renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
            Font::default(),
            Pixels(16.0),
        ));
        let mut tree = Tree::new(element.as_widget());
        let limits = layout::Limits::new(Size::ZERO, Size::new(canvas.0, canvas.1));
        let node = element.as_widget_mut().layout(&mut tree, &renderer, &limits);
        let bounds = node.bounds();
        (bounds.width, bounds.height)
    }

    /// Assert the video card keeps IDENTICAL outer dimensions (width,
    /// height) across every DownloadState — the media frame is always
    /// rendered and the state-conditional sections occupy fixed-height
    /// slots, so failure (or completion, or cancellation) never re-flows
    /// the card box.
    #[cfg(feature = "video-playback")]
    #[test]
    fn video_card_outer_bounds_are_identical_across_all_states() {
        let states = [
            DownloadState::Ready { total: Some(44_000_000) },
            DownloadState::Active {
                bytes: 19_000_000,
                total: Some(44_000_000),
            },
            DownloadState::Paused {
                bytes: 19_000_000,
                total: Some(44_000_000),
            },
            DownloadState::Completed {
                saved_name: "clip.mp4".into(),
                saved_path: None,
                total_size: Some(44_000_000),
            },
            DownloadState::Shared {
                name: "clip.mp4".into(),
                path: std::path::PathBuf::from("/tmp/clip.mp4"),
                size: Some(44_000_000),
            },
            DownloadState::Failed {
                failure: DownloadFailure::PeerOffline {
                    detail: Some("peer is offline right now".into()),
                },
            },
            DownloadState::Cancelled,
        ];

        let mut measured: Vec<(f32, f32)> = Vec::new();
        for state in states {
            let mut att = DownloadAttachment::new(TransferKind::Video, "clip.mp4", "ticket", "Duke", None);
            att.state = state;
            let card = BoruVideoFileCard::new(
                0,
                false,
                false,
                None,
                false,
                None,
                false,
                false,
                Some(1_800_000_000_000_i64),
                720.0,
            );
            // player=None → the returned element is 'static-compatible.
            let mut element: iced::Element<'static, AppMessage> = card.view(&att);
            measured.push(measure_outer_bounds(&mut element, (900.0, 1600.0)));
        }

        let (w0, h0) = measured[0];
        for (i, (w, h)) in measured.iter().enumerate() {
            assert!(
                (w - w0).abs() < 0.5,
                "state {i}: width {w} differs from Ready width {w0}"
            );
            assert!(
                (h - h0).abs() < 0.5,
                "state {i}: height {h} differs from Ready height {h0}"
            );
        }
        assert!(w0 > 200.0 && h0 > 200.0, "card box implausibly small: {w0}x{h0}");
    }
}
