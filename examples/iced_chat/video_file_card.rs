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
use iced::widget::{self, button, container, row, text, tooltip, Column, Row};
use iced::{Alignment, Color, Length};
#[cfg(feature = "video-playback")]
use iced_video_player::{Video, VideoPlayer};

use super::app::{
    accent_green, border_muted, color_error, text_system, SPACE_10, SPACE_12, SPACE_16, SPACE_2,
    SPACE_20, SPACE_24, SPACE_4, SPACE_6, SPACE_8, TYPO_SM, TYPO_XS, TYPO_XXS,
};
use super::app::{AppMessage, DownloadAttachment, DownloadState};
use super::download_progress_view::{
    action_button, action_buttons, active_download_detail, file_type_icon_element, human_size,
    progress_section, resolve_theme, secondary_button, state_badge, state_badge_color,
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

/// Compute the bounded media-frame size for the given intrinsic dimensions.
///
/// Unknown dimensions fall back to a 16:9 widescreen default (the spec's safe
/// default while metadata loads). The returned `(width, height)` always
/// preserves the exact intrinsic aspect ratio; the class bounds only pick a
/// sensible on-card footprint so portrait videos do not dominate the chat and
/// landscape videos may use most or all of the card width. There is no fixed
/// 16:9 crop — the frame is ratio-exact in every normal case and `contain`
/// letterboxes only when an extreme ratio collides with both caps.
fn media_frame_size(dimensions: Option<(u32, u32)>) -> (f32, f32) {
    let (width, height) = dimensions
        .filter(|(width, height)| *width > 0 && *height > 0)
        .map(|(width, height)| (width as f32, height as f32))
        .unwrap_or((16.0, 9.0));
    let ratio = width / height;
    let (max_width, max_height) = match aspect_ratio_class(ratio) {
        // VIDCARD-06 landscape: the frame may use most or all of the card
        // width — the spec's typical 16:9 preview is 720×405 px — with a
        // ~500 px height cap so near-square or unusual landscape files
        // cannot dominate the chat. Very wide videos follow the width bound
        // and their exact ratio, producing a short, wide frame instead of an
        // excessive height; the media is contained (never cropped) inside it.
        MediaAspectClass::Landscape => (720.0, 500.0),
        MediaAspectClass::Square => (480.0, 520.0),
        MediaAspectClass::Portrait => (380.0, 520.0),
    };

    // Start from the class's preferred width and derive the height that
    // preserves the exact ratio; if that exceeds the height cap, derive the
    // width from the cap instead. The result is always ratio-exact.
    let mut frame_width = max_width;
    let mut frame_height = frame_width / ratio;
    if frame_height > max_height {
        frame_height = max_height;
        frame_width = frame_height * ratio;
    }

    (frame_width, frame_height)
}

/// Bounded, ratio-exact media-frame sizing strategy (VIDCARD-08 / spec Task 8).
///
/// When the intrinsic dimensions are known *and* a poster thumbnail exists,
/// the frame is **responsive**: its width is `min(available, nominal_width)`
/// and the poster/player derive the height from the actual rendered width
/// (see the `height(Shrink)` + `ContentFit::Contain` widgets in
/// `media_frame`), so the exact source ratio is preserved and both
/// dimensions shrink proportionally at narrow window sizes — never
/// stretching, squashing or cropping, and never overflowing the chat column.
///
/// When dimensions are unknown (the spec's safe 16:9 default while metadata
/// loads) or no thumbnail exists yet, the frame stays **fixed** at the
/// nominal size so the bounded placeholder never collapses or inflates.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MediaFrameSizing {
    nominal_width: f32,
    nominal_height: f32,
    responsive: bool,
}

impl MediaFrameSizing {
    fn new(dimensions: Option<(u32, u32)>, has_thumbnail: bool) -> Self {
        let (nominal_width, nominal_height) = media_frame_size(dimensions);
        Self {
            nominal_width,
            nominal_height,
            responsive: dimensions.is_some() && has_thumbnail,
        }
    }

    /// Horizontal strategy: fill the available card width (capped by
    /// [`Self::max_width`]) when responsive, otherwise the fixed nominal box.
    fn width(&self) -> Length {
        if self.responsive {
            Length::Fill
        } else {
            Length::Fixed(self.nominal_width)
        }
    }

    /// Vertical strategy: derive the height from the actual rendered width
    /// (ratio-preserving) when responsive, otherwise the fixed nominal box.
    fn height(&self) -> Length {
        if self.responsive {
            Length::Shrink
        } else {
            Length::Fixed(self.nominal_height)
        }
    }

    /// `min(100%, nominal_width)` cap applied when responsive so a portrait
    /// frame never spans the full card width or the full chat column.
    fn max_width(&self) -> Option<f32> {
        self.responsive.then_some(self.nominal_width)
    }
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
            .push(text("Preparing…").size(TYPO_XS).color(ON_MEDIA_TEXT))
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

/// Compact duration badge for the lower-right corner of the media frame
/// (VIDCARD-11). Uses the live player's real duration metadata — there is
/// no other honest duration source in the transfer protocol — and is only
/// rendered when that duration is known and non-zero.
#[cfg(feature = "video-playback")]
fn duration_badge(duration: std::time::Duration) -> iced::Element<'static, AppMessage> {
    container(
        crate::fonts::type_role_text(
            crate::fonts::TypeRole::Metadata,
            format_media_time(duration),
        )
        .color(Color::WHITE),
    )
    .padding([SPACE_2, SPACE_6])
    .style(|_t| widget::container::Style {
        background: Some(iced::Background::Color(Color::from_rgba(
            0.0, 0.0, 0.0, 0.72,
        ))),
        border: iced::Border {
            radius: SPACE_6.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}
#[cfg(feature = "video-playback")]
fn format_media_time(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
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

/// Reserved width (px) on the right edge of the control bar so the
/// lower-right duration badge never covers the Expand control.
const DURATION_BADGE_ZONE: f32 = 64.0;

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
fn overflow_menu_item<'a>(label: &'a str, msg: AppMessage) -> iced::widget::Button<'a, AppMessage> {
    button(crate::fonts::type_role_text(
        crate::fonts::TypeRole::ButtonLabel,
        label,
    ))
    .on_press(msg)
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
    })
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
        received_at_ms: Option<i64>,
    ) -> Self {
        Self {
            entry_index,
            dark_mode,
            overflow_open,
            #[cfg(feature = "video-playback")]
            player,
            preparing,
            received_at_ms,
            #[cfg(feature = "video-playback")]
            seek_position,
            #[cfg(feature = "video-playback")]
            expanded,
            #[cfg(not(feature = "video-playback"))]
            _marker: std::marker::PhantomData,
        }
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
        let error_section = self.error_section(attachment, tone, muted, error_color);

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
        if let Some(err) = error_section {
            body = body.push(
                container(err)
                    .padding(SPACE_6)
                    .style(|t| widget::container::Style {
                        border: iced::Border {
                            color: {
                                let c = border_muted(t);
                                Color::from_rgba(c.r, c.g, c.b, 0.3)
                            },
                            width: 1.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
            );
        }
        body = body.spacing(SPACE_12);

        // VIDCARD-03 card surface: reuse the shared Boru card style —
        // soft white (theme-aware) background, thin neutral green-grey
        // border, RADIUS_CARD (16 px), very subtle shadow — with 20-24 px
        // internal padding. Width stays `Shrink` (content-driven; capped
        // by the readable chat column), so a portrait video with a bounded
        // media frame never forces the card to span the whole chat width.
        // No hidden overflow here: `.clip(true)` is only used at the
        // media-frame boundary to respect its rounded corners.
        container(body)
            .width(Length::Shrink)
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
        // surface, no per-screen extension maps.
        let video_icon =
            file_type_icon_element(&attachment.name, None, None, FileTypeIconSize::Card, theme);

        // Filename: single line, width-capped + clipped so a long name can
        // never widen the card. The tooltip exposes the full name and the
        // copy action in the overflow menu exposes it to the clipboard.
        let display_name = truncate_filename(&attachment.name, HEADER_FILENAME_MAX_CHARS);
        let filename = container(
            crate::fonts::type_role_text(crate::fonts::TypeRole::BodyEmphasised, display_name)
                .color(design_tokens::text_primary(theme))
                .wrapping(Wrapping::None),
        )
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

        if let Some(format) = file_format_label(&attachment.name) {
            title_row = title_row.push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, format)
                    .color(muted),
            );
        }

        title_row = title_row.push(
            tooltip::Tooltip::new(
                OverflowMenu::build(
                    AppMessage::ToggleVideoCardMenu(self.entry_index),
                    false,
                    theme,
                ),
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
        let sizing = MediaFrameSizing::new(
            attachment.poster_dimensions,
            attachment.thumbnail_handle.is_some(),
        );

        // Poster: the real thumbnail (contain, centred) or an honest
        // placeholder. While the poster is still being prepared (downloading
        // or verifying) show the loading indicator (VIDCARD-11).
        let poster: iced::Element<'static, AppMessage> =
            if let Some(ref handle) = attachment.thumbnail_handle {
                iced::widget::image(handle.clone())
                    // Contain: preserve the poster's exact intrinsic ratio,
                    // centred inside the frame — never stretch or crop. When
                    // the frame is responsive the image derives its height
                    // from the actual rendered width (Fill + Shrink), so the
                    // whole preview shrinks proportionally at narrow window
                    // sizes instead of overflowing (VIDCARD-08 Task 8).
                    .content_fit(iced::ContentFit::Contain)
                    .width(Length::Fill)
                    .height(if sizing.responsive {
                        Length::Shrink
                    } else {
                        Length::Fill
                    })
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
                let subtitle = media_placeholder_text(attachment);
                container(
                    Column::new()
                        .push(file_type_icon_element(
                            &attachment.name,
                            None,
                            None,
                            FileTypeIconSize::Large,
                            &resolve_theme(self.dark_mode),
                        ))
                        .push(text(subtitle).size(TYPO_XS).color(ON_MEDIA_TEXT))
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
        // circle), keyboard accessible (iced buttons focus and activate with
        // Enter/Space) and labelled "Play video" via the project's
        // icon-button Tooltip convention.
        let play = tooltip::Tooltip::new(
            button(
                Icon::Play
                    .build()
                    .size(IconSize::Xl)
                    .color_fn(|_| Color::WHITE)
                    .interactive(true)
                    .build(),
            )
            .on_press_maybe(
                (presentation == VideoPresentationState::Ready && !self.preparing)
                    .then_some(play_message),
            )
            .padding([(PLAY_OVERLAY_SIZE - IconSize::Xl.px()) / 2.0; 2])
            .style(|_theme, _status| widget::button::Style {
                background: Some(iced::Background::Color(MEDIA_FRAME_OVERLAY_BG)),
                border: iced::Border {
                    radius: (PLAY_OVERLAY_SIZE / 2.0).into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Play video"),
            tooltip::Position::Top,
        )
        .gap(SPACE_4);

        let error_preview = attachment.playback_error.as_ref().map(|error| {
            container(
                Column::new()
                    .push(text(error.title()).size(TYPO_SM).color(error_color))
                    .push(text(error.message()).size(TYPO_XS).color(ON_MEDIA_TEXT))
                    .push(
                        text("The original attachment is still available below.")
                            .size(TYPO_XXS)
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
            let mut frame = container(widget::stack![
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
            .style(media_frame_style);
            if let Some(max_width) = sizing.max_width() {
                frame = frame.max_width(max_width);
            }
            frame.into()
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
            let controls = Column::new()
                .push(
                    iced::widget::slider(0.0..=1.0, fraction, AppMessage::InlineVideoSeekChanged)
                        .on_release(AppMessage::InlineVideoSeekReleased)
                        .step(0.001_f32)
                        .width(Length::Fill),
                )
                .push(
                    Row::new()
                        .push(action_button(
                            if video.paused() { "Play" } else { "Pause" },
                            AppMessage::PlayInlineVideo(self.entry_index),
                        ))
                        .push(
                            text(format!(
                                "{} / {}",
                                format_media_time(position),
                                format_media_time(duration),
                            ))
                            .size(TYPO_XS)
                            .color(Color::WHITE),
                        )
                        .push(action_button(
                            if video.muted() { "Unmute" } else { "Mute" },
                            AppMessage::InlineVideoToggleMute,
                        ))
                        .push(
                            iced::widget::slider(
                                0.0..=1.0,
                                video.volume() as f32,
                                AppMessage::InlineVideoSetVolume,
                            )
                            .step(0.01_f32)
                            .width(Length::Fixed(90.0)),
                        )
                        .push(action_button(
                            if self.expanded { "Collapse" } else { "Expand" },
                            AppMessage::InlineVideoToggleExpanded,
                        ))
                        .spacing(SPACE_6)
                        .align_y(Alignment::Center),
                );
            // Task 10: the playing element occupies the exact same media box
            // as the poster — no layout jump when Play is pressed. The video
            // is contained (never stretched or cropped) and the controls
            // overlay the frame's bottom edge on the existing translucent
            // dark surface, so poster and player share width, height, aspect
            // ratio, border radius and position. When the frame is
            // responsive the player derives its height from the actual
            // rendered width just like the poster; the zero-size guard keeps
            // the fixed nominal frame until the video reports real dims.
            let (video_w, video_h) = video.size();
            let player_sizing = if video_w > 0 && video_h > 0 {
                sizing
            } else {
                MediaFrameSizing {
                    nominal_width: sizing.nominal_width,
                    nominal_height: sizing.nominal_height,
                    responsive: false,
                }
            };
            let video_element: iced::Element<'a, AppMessage> = if player_sizing.responsive {
                VideoPlayer::new(&video)
                    .content_fit(iced::ContentFit::Contain)
                    .on_end_of_stream(AppMessage::CloseInlineVideo)
                    .on_error(|_error| AppMessage::CloseInlineVideo)
                    .width(Length::Fill)
                    .height(Length::Shrink)
                    .into()
            } else {
                container(
                    VideoPlayer::new(&video)
                        .content_fit(iced::ContentFit::Contain)
                        .on_end_of_stream(AppMessage::CloseInlineVideo)
                        .on_error(|_error| AppMessage::CloseInlineVideo),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
            };

            let controls_bar = container(controls)
                .padding([SPACE_6, SPACE_8])
                .style(|_theme| widget::container::Style {
                    background: Some(iced::Background::Color(Color::from_rgba(
                        0.0, 0.0, 0.0, 0.76,
                    ))),
                    ..Default::default()
                });

            // VIDCARD-11 duration badge: lower-right corner of the media
            // frame, real player metadata only, shown only when the duration
            // is actually known (non-zero). The control bar's right zone is
            // reserved so the badge never covers the Expand control.
            let badge_known = duration.as_secs() > 0;
            let badge_zone = if badge_known {
                DURATION_BADGE_ZONE
            } else {
                0.0
            };
            let badge_layer: iced::Element<'static, AppMessage> = if badge_known {
                duration_badge(duration)
            } else {
                iced::widget::Space::new().width(0.0).height(0.0).into()
            };

            let mut player_frame = container(widget::stack![
                video_element,
                container(controls_bar)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_y(Alignment::End)
                    .padding(iced::Padding::new(0.0).right(badge_zone)),
                container(badge_layer)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::End)
                    .align_y(Alignment::End)
                    .padding(iced::Padding::new(0.0).right(SPACE_8).bottom(SPACE_8)),
            ])
            .width(player_sizing.width())
            .height(player_sizing.height())
            .clip(true)
            .style(media_frame_style);
            if let Some(max_width) = player_sizing.max_width() {
                player_frame = player_frame.max_width(max_width);
            }
            player_frame.into()
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
        if !groups.is_empty() {
            column = column.push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    groups.join("  ·  "),
                )
                .color(muted)
                .wrapping(Wrapping::Word)
                .width(Length::Fill),
            );
        }

        if let Some(prog) = progress_section(state, self.dark_mode) {
            column = column.push(prog);
        }
        // VIDCARD-14: bytes of total, percentage and transfer speed — only
        // where the transfer layer provides them (no invented estimates).
        // Active uses the green progress accent; paused uses the muted tone.
        if let Some(detail) = active_download_detail(attachment) {
            let detail_color = if matches!(state, DownloadState::Paused { .. }) {
                muted
            } else {
                accent_green(theme)
            };
            column = column.push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, detail)
                    .color(detail_color),
            );
        }

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
        column = column.push(action_row);

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

    // ── Failure details ───────────────────────────────────────────────

    fn error_section(
        &self,
        attachment: &DownloadAttachment,
        tone: Color,
        muted: Color,
        error_color: Color,
    ) -> Option<iced::Element<'a, AppMessage>> {
        match &attachment.state {
            DownloadState::Failed { failure } => {
                let mut column = Column::new()
                    .push(
                        row![
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::BodyEmphasised,
                                failure.title(),
                            )
                            .color(error_color),
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Metadata,
                                failure.stability_label(),
                            )
                            .color(tone),
                        ]
                        .spacing(SPACE_8)
                        .align_y(Alignment::Center),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            failure.message(),
                        )
                        .color(muted)
                        .width(Length::Fill),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            format!("Recovery: {}", failure.recovery_action()),
                        )
                        .color(tone)
                        .width(Length::Fill),
                    );

                if let Some(detail) = failure.diagnostics() {
                    if !detail.is_empty() {
                        column = column.push(
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::TechnicalValue,
                                detail,
                            )
                            .color(muted)
                            .width(Length::Fill),
                        );
                    }
                }

                Some(column.into())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        aspect_ratio_class, file_format_label, format_relative_time, header_badge, media_frame_size,
        media_placeholder_text, truncate_filename, video_presentation_state, MediaAspectClass,
        MediaFrameSizing, VideoPresentationState, HEADER_FILENAME_MAX_CHARS,
    };
    use iced::Length;
    use crate::app::{DownloadAttachment, DownloadFailure, DownloadState, TransferKind};
    use std::path::PathBuf;

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
        let (width, height) = media_frame_size(None);
        assert_eq!(width, 720.0);
        assert!((height - 405.0).abs() < 0.01);
    }

    #[test]
    fn landscape_frame_preserves_exact_intrinsic_ratio() {
        // 16:9 fills the landscape width bound; the height derives from the
        // exact ratio (no fixed 16:9 crop, no stretch/squash).
        let (width, height) = media_frame_size(Some((3840, 2160)));
        assert_eq!(width, 720.0);
        assert!((height - 405.0).abs() < 0.01);
        assert!((width / height - 3840.0 / 2160.0).abs() < 1e-6);
    }

    #[test]
    fn landscape_typical_hd_preview_matches_spec() {
        // VIDCARD-06 spec: a typical 16:9 preview is approximately
        // 720×405 px where space allows. 1280×720 derives exactly that.
        let (width, height) = media_frame_size(Some((1280, 720)));
        assert_eq!(width, 720.0);
        assert!((height - 405.0).abs() < 0.01);
        assert!((width / height - 1280.0 / 720.0).abs() < 1e-6);
    }

    #[test]
    fn landscape_frame_caps_height_for_near_square_ratios() {
        // 4:3 landscape would exceed the ~500 px height cap at the full
        // landscape width, so the width derives down from the cap — the
        // result stays ratio-exact and never dominates the chat.
        let (width, height) = media_frame_size(Some((640, 480)));
        assert_eq!(height, 500.0);
        assert!((width - 666.6667).abs() < 0.01);
        assert!((width / height - 640.0 / 480.0).abs() < 1e-6);
    }

    #[test]
    fn square_frame_uses_bounded_square_footprint() {
        let (width, height) = media_frame_size(Some((1080, 1080)));
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
        let (width, height) = media_frame_size(Some((1080, 1200)));
        assert_eq!(height, 520.0);
        assert!(
            (width - 468.0).abs() < 0.01,
            "width {width} should derive to preserve ratio"
        );
        assert!((width / height - 1080.0 / 1200.0).abs() < 1e-6);

        // 1200x1080 (ratio 1.111) is near-square but slightly wide: the
        // preferred width cap (480) wins and the height derives to keep the
        // exact ratio — again no forced perfect square.
        let (width2, height2) = media_frame_size(Some((1200, 1080)));
        assert_eq!(width2, 480.0);
        assert!((width2 / height2 - 1200.0 / 1080.0).abs() < 1e-6);
    }

    #[test]
    fn square_frame_preferred_width_stays_in_spec_band() {
        // VIDCARD-07 spec: preferred width 420-560 px for square videos.
        // A perfect 1:1 uses the class preferred width directly.
        let (width, _height) = media_frame_size(Some((1080, 1080)));
        assert!(
            (420.0..=560.0).contains(&width),
            "square preferred width {width} must stay in the 420-560 px band"
        );
    }

    #[test]
    fn square_frame_max_height_is_bounded() {
        // VIDCARD-07 spec: maximum height ~520 px. Near-square frames that
        // hit the height cap must still keep the exact ratio.
        let (width, height) = media_frame_size(Some((1080, 1200)));
        assert!(height <= 520.0 + 1e-6);
        assert!((width / height - 0.9).abs() < 1e-6);
    }

    #[test]
    fn square_media_frame_is_centred_and_width_capped_not_stretched() {
        // VIDCARD-07: the square preview must feel intentionally centred,
        // not like a landscape frame containing a small square on the left.
        // The media element is wrapped in a Fill-width container that centres
        // it (`center_x(Fill)`), and the frame itself is width-capped with a
        // Fixed preview width — it never stretches to the full card width.
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();

        // The body column wraps the media in a centring container.
        let body = prod
            .split("let mut body = Column::new()")
            .nth(1)
            .and_then(|s| s.split(".width(Length::Shrink)").next())
            .expect("card body column block must exist");
        assert!(
            body.contains("container(media).width(Length::Fill).center_x(Length::Fill)"),
            "square media frame must be centred via a Fill wrapper + center_x(Fill)"
        );

        // The media frame itself is width-capped (never plain Fill): the
        // shared responsive sizing strategy (VIDCARD-08) derives the frame
        // width from `MediaFrameSizing` — `Fill` bounded by `max_width` =
        // the nominal width — so a square preview stays centred at its
        // capped size and never stretches to the full card width, while
        // still shrinking proportionally at very narrow windows (Task 8).
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
            media_frame.contains("sizing.max_width()"),
            "media frame must be width-capped via max_width (never stretch to full card width)"
        );
        assert!(
            media_frame.contains(".height(sizing.height())"),
            "media frame height must come from the bounded sizing strategy"
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
        let (width, height) = media_frame_size(Some((720, 1280)));
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
            let (frame_w, frame_h) = media_frame_size(Some((width, height)));
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
        let (frame_w, frame_h) = media_frame_size(Some((720, 1280)));
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
    fn media_frame_sizing_is_responsive_with_known_dims_and_thumbnail() {
        // Known intrinsic dimensions + a poster thumbnail → the frame fills
        // the available width (capped at the nominal width) and derives its
        // height from the actual rendered width, so it shrinks
        // proportionally at narrow window sizes instead of overflowing.
        let sizing = MediaFrameSizing::new(Some((720, 1280)), true);
        assert!(sizing.responsive);
        assert_eq!(sizing.width(), Length::Fill);
        assert_eq!(sizing.height(), Length::Shrink);
        assert_eq!(sizing.max_width(), Some(292.5));
    }

    #[test]
    fn media_frame_sizing_stays_fixed_without_thumbnail_or_dims() {
        // Unknown dimensions (spec's safe 16:9 default while metadata loads)
        // → fixed bounded frame, never a frame with no size driver. The
        // fallback tracks the landscape width bound (VIDCARD-06: 720×405).
        let unknown = MediaFrameSizing::new(None, true);
        assert!(!unknown.responsive);
        assert_eq!(unknown.max_width(), None);
        assert_eq!(unknown.width(), Length::Fixed(720.0));
        assert_eq!(unknown.height(), Length::Fixed(405.0));

        // Known dimensions but no thumbnail yet (poster still generating) →
        // fixed nominal frame so the placeholder cannot collapse or inflate.
        let no_thumb = MediaFrameSizing::new(Some((720, 1280)), false);
        assert!(!no_thumb.responsive);
        assert_eq!(no_thumb.width(), Length::Fixed(292.5));
        assert_eq!(no_thumb.height(), Length::Fixed(520.0));
    }

    #[test]
    fn media_frame_sizing_nominal_matches_media_frame_size() {
        let sizing = MediaFrameSizing::new(Some((3840, 2160)), true);
        let (width, height) = media_frame_size(Some((3840, 2160)));
        assert_eq!(sizing.nominal_width, width);
        assert_eq!(sizing.nominal_height, height);
    }

    #[test]
    fn media_frame_uses_neutral_dark_background_and_responsive_shrink() {
        // VIDCARD-08 Task 8: portrait previews sit on a fixed neutral dark
        // media background (letterboxing reads as deliberate) and the frame
        // caps its width at the nominal size while the poster derives its
        // height from the actual rendered width — no full-card stretch, no
        // top/bottom crop, no horizontal overflow at narrow window sizes.
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
            frame.contains("sizing.max_width()"),
            "responsive frame must cap width via max_width (min(100%, nominal))"
        );
        assert!(
            frame.contains("ContentFit::Contain"),
            "poster/player must render contain-style (never stretch or crop)"
        );
        assert!(
            frame.contains("Length::Shrink"),
            "poster/player must derive height from the actual rendered width when responsive"
        );
    }

    #[test]
    fn ultrawide_frame_stays_bounded_and_ratio_exact() {
        // 21:9 uses the full landscape width; the height follows the exact
        // ratio instead of forcing a 16:9 box — a short, wide frame with no
        // excessive vertical height and nothing cropped.
        let (width, height) = media_frame_size(Some((6720, 2880)));
        assert_eq!(width, 720.0);
        assert!((height - 308.571).abs() < 0.01);
        assert!((width / height - 6720.0 / 2880.0).abs() < 1e-6);
    }

    #[test]
    fn ultrawide_panorama_stays_short_and_ratio_exact() {
        // 32:9 panorama: still the full landscape width, very short frame —
        // the contain rule keeps every pixel visible (no side cropping).
        let (width, height) = media_frame_size(Some((7680, 2160)));
        assert_eq!(width, 720.0);
        assert!((height - 202.5).abs() < 0.01);
        assert!((width / height - 7680.0 / 2160.0).abs() < 1e-6);
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
        let (width, height) = media_frame_size(None);
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
            outer.contains(".width(Length::Shrink)"),
            "card width must be content-driven (Shrink), not forced full width"
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

    #[test]
    fn play_overlay_is_circular_high_contrast_and_has_accessible_label() {
        // VIDCARD-11: the play overlay must be a centred, circular,
        // semi-transparent dark button with a strong-contrast glyph, a
        // keyboard-accessible button widget, and an accessible label such
        // as "Play video".
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
            "play overlay must be a real button (keyboard accessible)"
        );
    }

    #[test]
    fn duration_badge_uses_real_metadata_only() {
        // VIDCARD-11: the duration badge must come from real player
        // metadata, appear only when the duration is known (non-zero), and
        // sit in the lower-right corner of the media frame.
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let media_frame_fns = prod
            .split("fn media_frame(")
            .nth(1)
            .expect("media_frame must exist");

        assert!(
            media_frame_fns.contains("duration.as_secs() > 0"),
            "duration badge must only appear when the duration is known"
        );
        assert!(
            media_frame_fns.contains("duration_badge(duration)"),
            "badge content must come from the real player duration"
        );
        assert!(
            media_frame_fns.contains("align_x(Alignment::End)")
                && media_frame_fns.contains("align_y(Alignment::End)"),
            "duration badge must sit in the lower-right corner"
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
        // preserve that on top of VIDCARD-08's responsive sizing: both the
        // poster frame and the player frame are driven by the same
        // MediaFrameSizing (sizing / player_sizing) and share the same
        // media-frame surface style and boundary clip.
        let src = include_str!("video_file_card.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let media_frame_fns = prod
            .split("fn media_frame(")
            .nth(1)
            .expect("media_frame must exist");

        // Both the poster preview and the player use the shared
        // MediaFrameSizing system (poster: sizing; player: player_sizing).
        assert!(
            media_frame_fns.contains("MediaFrameSizing::new("),
            "poster frame must be sized by the shared MediaFrameSizing"
        );
        assert!(
            media_frame_fns.contains("player_sizing"),
            "player frame must be sized by the player_sizing variant"
        );
        assert!(
            media_frame_fns.matches(".style(media_frame_style)").count() >= 2,
            "poster and player frames must use the same shared surface style"
        );
        assert!(
            media_frame_fns.matches(".clip(true)").count() >= 2,
            "poster and player frames must both clip overflow at the frame boundary"
        );
    }
}
