//! Download progress widget — a stateless card rendering a single download row.
//!
//! This module provides [`view_download_progress`], a stateless widget that
//! renders a `DownloadAttachment` as a compact card with:
//!
//! - State badge (text + colour) indicating the current download status
//! - Filename and human-readable total size in the header row
//! - Source peer label and optional transfer speed
//! - Modern progress bar (thin rounded track, green fill) with percentage
//!   for active/paused states, plus a bytes-of-total / percentage / speed
//!   detail line for active downloads (real transfer data only — no
//!   invented estimates)
//! - Context-sensitive action buttons (pause/resume/cancel/retry/open)
//! - Prominent failure reason in the Failed state
//!
//! All colors, spacing, and typography use the existing constants from the
//! parent module to stay consistent with the app's design system.
//!
//! ## File-type icons (PAPIRUS-10)
//!
//! Chat file cards render their file-type icon through the central
//! [`crate::file_type_icon::FileTypeIcon`] component (resolved by the
//! central resolver).  Because `FileTypeIcon::build` returns an element tied
//! to the configured icon's borrow, and chat views return `'static` elements
//! (through `iced::widget::lazy`), this module keeps a process-global cache
//! of leaked icon configurations keyed by filename/MIME/size — the same
//! process-lifetime strategy the component itself uses for SVG handles.
//! [`file_type_icon_element`] is the single chat-side entry point; no chat
//! surface keeps its own extension→icon map.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use iced::widget::text::Wrapping;
use iced::widget::{self, button, container, row, Column, Row};
use iced::{Alignment, Color, Length};
#[cfg(feature = "video-playback")]
use iced_video_player::Video;

use super::app::{
    icon_svg, AppMessage, DownloadAttachment, DownloadState, ICON_COPY, ICON_FILES, ICON_FOLDER,
    ICON_MESH, ICON_PLAY, ICON_RETRY,
};
use crate::file_type_icon::{FileTypeIcon, FileTypeIconSize};

// Re-import the design-token helpers and constants from app.rs.
use super::app::{
    accent_green, accent_primary, bg_surface, border_muted, color_error, text_muted, text_system,
    SPACE_10, SPACE_12, SPACE_16, SPACE_2, SPACE_4, SPACE_6, SPACE_8, TYPO_XS,
};

// ── Progress bar geometry (VIDCARD-14) ────────────────────────────────

/// Height (px) of the thin modern progress-bar track.
const PROGRESS_BAR_GIRTH: f32 = 6.0;

/// Fixed width (px) of the percentage label next to the bar.  Holding the
/// label width constant means the bar itself never re-measures as the value
/// climbs 0% → 100% (no rapid layout changes).
const PROGRESS_PCT_LABEL_WIDTH: f32 = 44.0;

// ── Theme dispatch (light/dark) ──────────────────────────────────────────

/// Resolve the active Iced theme from the dark-mode flag.
pub(crate) fn resolve_theme(dark_mode: bool) -> iced::Theme {
    if dark_mode {
        iced::Theme::Dark
    } else {
        iced::Theme::Light
    }
}

/// Colour keyed to the current download state — used for the state badge.
pub(crate) fn state_badge_color(state: &DownloadState, theme: &iced::Theme) -> Color {
    match state {
        DownloadState::Ready { .. }
        | DownloadState::Active { .. }
        | DownloadState::Paused { .. } => accent_primary(theme),
        DownloadState::Completed { .. } => accent_green(theme),
        DownloadState::Shared { .. } => accent_primary(theme),
        DownloadState::Failed { failure } => match failure.stability_label() {
            "Temporary" => Color::from_rgb(0.78, 0.58, 0.16),
            "Terminal" | "Permanent" => color_error(theme),
            _ => color_error(theme),
        },
        DownloadState::Cancelled => Color::from_rgb(0.55, 0.55, 0.55),
    }
}

/// Short human-readable label for each state (shown in the badge).
pub(crate) fn state_badge_label(state: &DownloadState) -> String {
    match state {
        DownloadState::Ready { .. } => "Pending".to_string(),
        DownloadState::Active { .. } => "Downloading".to_string(),
        DownloadState::Paused { .. } => "Paused".to_string(),
        DownloadState::Completed { .. } => "Complete".to_string(),
        DownloadState::Shared { .. } => "Shared".to_string(),
        DownloadState::Failed { failure } => failure.stability_label().to_string(),
        DownloadState::Cancelled => "Cancelled".to_string(),
    }
}

// ── File-type icon element (PAPIRUS-10) ─────────────────────────────────

/// Cache key for a configured file-type icon.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileTypeIconKey {
    filename: String,
    mime_type: Option<String>,
    detected_type: Option<String>,
    size: FileTypeIconSize,
    /// Explicit folder state (PAPIRUS-12). Never inferred from the filename;
    /// a directory named "report.pdf" must not collide with a file of the
    /// same name, so the flag is part of the key.
    is_directory: bool,
    /// Purely decorative icon (PAPIRUS-15): hidden from assistive
    /// technology.  Part of the key so an informative and a decorative
    /// rendering of the same file never share a cached configuration.
    decorative: bool,
    /// Whether the icon opts into a hover tooltip with the friendly type.
    show_tooltip: bool,
}

impl std::hash::Hash for FileTypeIconKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.filename.hash(state);
        self.mime_type.hash(state);
        self.detected_type.hash(state);
        // FileTypeIconSize does not derive Hash; its Papirus size directory is
        // injective (16/24/32/48/64 map one-to-one), so hash that instead.
        self.size.papirus_dir().hash(state);
        self.is_directory.hash(state);
        self.decorative.hash(state);
        self.show_tooltip.hash(state);
    }
}

/// Process-global cache of configured [`FileTypeIcon`] configurations.
///
/// `FileTypeIcon::build` returns an element tied to the `&self` borrow of the
/// configured icon, but chat card views return `'static` elements (they are
/// routed through `iced::widget::lazy`).  To satisfy both, we cache one
/// leaked `'static` `FileTypeIcon` per (filename, mime, detected, size) key —
/// the same process-lifetime strategy the component itself uses for its SVG
/// handle cache.  The leak is bounded by the number of distinct attachment
/// names a session sees (chat history is capped), and each entry is a tiny
/// resolved-icon config, not decoded SVG data.
static FILE_TYPE_ICON_CACHE: OnceLock<
    Mutex<HashMap<FileTypeIconKey, &'static FileTypeIcon<'static>>>,
> = OnceLock::new();

/// Build a Papirus file-type icon element for a chat card.
///
/// This is the single chat-side entry point to the central component
/// ([`FileTypeIcon`], resolved by [`crate::file_type_resolver`]).  Chat
/// surfaces pass the attachment name (and any MIME they already hold) and a
/// semantic size; they must NOT keep their own extension maps.
///
/// File displays call this function.  Folder displays call
/// [`directory_icon_element`] — the explicit-directory counterpart — so a
/// folder can never be mistaken for a file of the same name (PAPIRUS-12).
///
/// The returned icon is **informative** (PAPIRUS-15): it carries the
/// friendly accessible description derived from the resolved type.  Use
/// [`decorative_file_type_icon_element`] when the icon is purely
/// decorative next to a filename/type label already in the row, and
/// [`file_type_icon_element_with_tooltip`] to also surface the friendly
/// type in a hover tooltip.
pub(crate) fn file_type_icon_element(
    filename: &str,
    mime_type: Option<&str>,
    detected_type: Option<&str>,
    size: FileTypeIconSize,
    theme: &iced::Theme,
) -> iced::Element<'static, AppMessage> {
    file_type_icon_element_impl(
        filename,
        mime_type,
        detected_type,
        false,
        size,
        theme,
        false,
        false,
    )
}

/// Build a Papirus file-type icon element for a chat card, marked
/// **decorative** (PAPIRUS-15).
///
/// Use when the icon sits next to text that already carries the primary
/// content label (the filename) and the type is also stated (e.g. a
/// dashboard row whose metadata line prints the MIME/type label).  A
/// decorative icon is hidden from assistive technology: it contributes no
/// accessible name and never renders a type tooltip.
pub(crate) fn decorative_file_type_icon_element(
    filename: &str,
    mime_type: Option<&str>,
    detected_type: Option<&str>,
    size: FileTypeIconSize,
    theme: &iced::Theme,
) -> iced::Element<'static, AppMessage> {
    file_type_icon_element_impl(
        filename,
        mime_type,
        detected_type,
        false,
        size,
        theme,
        true,
        false,
    )
}

/// Build a Papirus file-type icon element for a chat card, **informative**
/// and with a hover tooltip showing the friendly type (PAPIRUS-15 point 7).
///
/// Use for icons that are the primary type signal in a card/header (the
/// filename stays the primary content label; the tooltip is supporting
/// information, never colour-alone).  Decorative callers must use
/// [`decorative_file_type_icon_element`] instead.
pub(crate) fn file_type_icon_element_with_tooltip(
    filename: &str,
    mime_type: Option<&str>,
    detected_type: Option<&str>,
    size: FileTypeIconSize,
    theme: &iced::Theme,
) -> iced::Element<'static, AppMessage> {
    file_type_icon_element_impl(
        filename,
        mime_type,
        detected_type,
        false,
        size,
        theme,
        false,
        true,
    )
}

/// Build a Papirus **folder** icon element (PAPIRUS-12).
///
/// The folder is resolved through the same central resolver as every file
/// display (priority 1: explicit directory state → `folder-open`) and
/// rendered through the same central [`FileTypeIcon`] component.  Callers
/// must pass explicit directory state from the application model; a folder
/// is never inferred from a filename ending in `/`.
///
/// Boru's transfer model is file-based today (the secure catalogue shares
/// individual files; folder sharing is a documented limitation surfaced by
/// `SharedFolderPicked`), so no row currently renders a folder.  This is
/// the folder-display entry point for the surfaces PAPIRUS-12 covers
/// (shared folders, folder transfer summaries, folders in Shared by Me /
/// Shared with Me, re-shared folders, folder activity entries): the moment
/// a row carries explicit directory state it must call this function so it
/// resolves through the central resolver/component like every file icon.
#[allow(dead_code)]
pub(crate) fn directory_icon_element(
    name: &str,
    size: FileTypeIconSize,
    theme: &iced::Theme,
) -> iced::Element<'static, AppMessage> {
    file_type_icon_element_impl(name, None, None, true, size, theme, false, false)
}

/// Shared implementation behind [`file_type_icon_element`] and
/// [`directory_icon_element`].  `is_directory` is explicit state from the
/// application model and is part of the cache key, so a folder named
/// `report.pdf` and a file named `report.pdf` never share an entry.
/// `decorative` and `show_tooltip` (PAPIRUS-15) are also part of the key
/// so an informative and a decorative rendering of the same file never
/// share a cached configuration.
fn file_type_icon_element_impl(
    filename: &str,
    mime_type: Option<&str>,
    detected_type: Option<&str>,
    is_directory: bool,
    size: FileTypeIconSize,
    theme: &iced::Theme,
    decorative: bool,
    show_tooltip: bool,
) -> iced::Element<'static, AppMessage> {
    let key = FileTypeIconKey {
        filename: filename.to_string(),
        mime_type: mime_type.map(str::to_string),
        detected_type: detected_type.map(str::to_string),
        size,
        is_directory,
        decorative,
        show_tooltip,
    };
    let cache = FILE_TYPE_ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap();
    if let Some(icon) = cache.get(&key) {
        return icon.build(theme);
    }
    // Leak the configured icon (and the strings it borrows) so the returned
    // element can be `'static`.  Bounded per unique key; see struct docs.
    // `Box::leak` yields `&'static mut str`; coerce to the shared reference
    // the component requires.
    let filename: &'static str = Box::leak(key.filename.clone().into_boxed_str());
    let mime_type: Option<&'static str> = key
        .mime_type
        .as_deref()
        .map(|m| Box::leak(m.to_string().into_boxed_str()) as &'static str);
    let detected_type: Option<&'static str> = key
        .detected_type
        .as_deref()
        .map(|m| Box::leak(m.to_string().into_boxed_str()) as &'static str);
    let mut icon_cfg =
        FileTypeIcon::new(filename, mime_type, detected_type, is_directory).size(key.size);
    if decorative {
        icon_cfg = icon_cfg.decorative();
    }
    if show_tooltip {
        icon_cfg = icon_cfg.with_tooltip();
    }
    let icon: &'static FileTypeIcon<'static> = Box::leak(Box::new(icon_cfg));
    cache.insert(key, icon);
    icon.build(theme)
}

// ── Human-readable byte formatting ───────────────────────────────────────

/// Format a byte count into a human-readable string (e.g., "4.2 MiB").
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut idx = 0usize;
    while value >= 1024.0 && idx < UNITS.len() - 1 {
        value /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{} {}", bytes, UNITS[idx])
    } else {
        format!("{:.1} {}", value, UNITS[idx])
    }
}

// ── State badge pill ─────────────────────────────────────────────────────

pub(crate) fn state_badge(state: &DownloadState, tone: Color) -> iced::widget::Container<'static, AppMessage> {
    container(
        crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, state_badge_label(state))
            .color(
                // Use a perceptually balanced off-white against the badge color
                Color::from_rgb(0.95, 0.95, 0.95),
            ),
    )
    .padding([SPACE_2, SPACE_6])
    .style(move |_t| widget::container::Style {
        background: Some(iced::Background::Color(tone)),
        border: iced::Border {
            radius: SPACE_10.into(),
            ..Default::default()
        },
        ..Default::default()
    })
}

// ── Action buttons ───────────────────────────────────────────────────────

/// A small ghost-style button with a compact outline.
///
/// Returns a keyboard-focusable button (spec Task 17): iced 0.14 buttons
/// have no `operation::Focusable` impl and cannot be Tab-reached or
/// activated with Enter/Space on their own, so every action button is
/// wrapped in [`crate::focusable_button::FocusableButton`], which joins the
/// app's focus traversal, activates on Enter/Space and draws a visible
/// focus ring.
pub(crate) fn action_button<'a>(label: &'a str, msg: AppMessage) -> iced::Element<'a, AppMessage> {
    let lbl = crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, label);
    crate::focusable_button::focusable_button(
        button(lbl)
            .on_press(msg.clone())
            .padding([SPACE_4, SPACE_10])
            .style(|theme, status| {
                let base = match status {
                    widget::button::Status::Hovered => accent_primary(theme),
                    widget::button::Status::Pressed => {
                        let mut c = accent_primary(theme);
                        c.r *= 0.85;
                        c.g *= 0.85;
                        c.b *= 0.85;
                        c
                    }
                    _ => Color::from_rgb(0.5, 0.5, 0.5),
                };
                widget::button::Style {
                    text_color: base,
                    background: None,
                    border: iced::Border {
                        color: border_muted(theme),
                        width: 1.0,
                        radius: SPACE_6.into(),
                    },
                    ..Default::default()
                }
            }),
        Some(msg),
    )
    .ring_radius(SPACE_6)
    .build()
}

/// A subtle text-only button (borderless, uses muted/destructive colour).
pub(crate) fn text_button<'a>(label: &'a str, msg: AppMessage) -> iced::Element<'a, AppMessage> {
    let lbl = crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, label);
    crate::focusable_button::focusable_button(
        button(lbl)
            .on_press(msg.clone())
            .padding([SPACE_4, SPACE_8])
            .style(|theme, status| {
                let base = match status {
                    widget::button::Status::Hovered => {
                        let mut c = color_error(theme);
                        c.a = 0.8;
                        c
                    }
                    widget::button::Status::Pressed => color_error(theme),
                    _ => Color::from_rgb(0.45, 0.45, 0.45),
                };
                widget::button::Style {
                    text_color: base,
                    background: None,
                    border: iced::Border {
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

/// Build the inner content of an action button: an optional leading icon
/// plus the label.  The icon is coloured with `icon_color(theme)` so it
/// tracks the surrounding theme (white on the green primary fill, the
/// system text colour on bordered secondary buttons).
fn action_content<'a>(
    icon: Option<&'static [u8]>,
    label: &'a str,
    icon_color: fn(&iced::Theme) -> Color,
) -> iced::Element<'a, AppMessage> {
    let text_el = crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, label);
    match icon {
        Some(svg_bytes) => row![
            icon_svg(svg_bytes, TYPO_XS)
                .style(move |t, _s| iced::widget::svg::Style { color: Some(icon_color(t)) }),
            text_el,
        ]
        .spacing(SPACE_4)
        .align_y(Alignment::Center)
        .into(),
        None => text_el.into(),
    }
}

/// Green filled primary action button (the single main action per state).
///
/// Keyboard-focusable: wrapped in [`crate::focusable_button::FocusableButton`]
/// so Tab traversal reaches it and Enter/Space activates it (spec Task 17).
pub(crate) fn primary_button<'a>(
    icon: Option<&'static [u8]>,
    label: &'a str,
    msg: AppMessage,
) -> iced::Element<'a, AppMessage> {
    crate::focusable_button::focusable_button(
        button(action_content(icon, label, |_t| Color::WHITE))
            .on_press(msg.clone())
            .padding([SPACE_6, SPACE_12])
            .style(super::app::BUTTON_PRIMARY_GREEN),
        Some(msg),
    )
    .ring_radius(crate::design_tokens::RADIUS_SM)
    .build()
}

/// Light bordered secondary action button (supporting actions per state).
///
/// Keyboard-focusable: wrapped in [`crate::focusable_button::FocusableButton`]
/// so Tab traversal reaches it and Enter/Space activates it (spec Task 17).
pub(crate) fn secondary_button<'a>(
    icon: Option<&'static [u8]>,
    label: &'a str,
    msg: AppMessage,
) -> iced::Element<'a, AppMessage> {
    crate::focusable_button::focusable_button(
        button(action_content(icon, label, text_system))
            .on_press(msg.clone())
            .padding([SPACE_6, SPACE_12])
            .style(|theme, status| {
                let base = match status {
                    widget::button::Status::Hovered => accent_primary(theme),
                    widget::button::Status::Pressed => {
                        let mut c = accent_primary(theme);
                        c.r *= 0.85;
                        c.g *= 0.85;
                        c.b *= 0.85;
                        c
                    }
                    _ => text_system(theme),
                };
                widget::button::Style {
                    text_color: base,
                    background: None,
                    border: iced::Border {
                        color: border_muted(theme),
                        width: 1.0,
                        radius: SPACE_6.into(),
                    },
                    ..Default::default()
                }
            }),
        Some(msg),
    )
    .ring_radius(SPACE_6)
    .build()
}

/// Disabled / loading button — no press handler, muted styling, and NOT
/// part of the keyboard focus order (no action to activate).
pub(crate) fn disabled_button<'a>(label: &'a str) -> iced::Element<'a, AppMessage> {
    let lbl = crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, label);
    crate::focusable_button::focusable_button(
        button(lbl)
            .padding([SPACE_6, SPACE_12])
            .style(|theme, _status| widget::button::Style {
                text_color: text_muted(theme),
                background: None,
                border: iced::Border {
                    color: border_muted(theme),
                    width: 1.0,
                    radius: SPACE_6.into(),
                },
                ..Default::default()
            }),
        None,
    )
    .ring_radius(SPACE_6)
    .build()
}

// ── Primary entry point ──────────────────────────────────────────────────

/// Render a complete download progress card for a single download row.
///
/// This is a stateless widget: given an `attachment` reference and an entry
/// index for routing action messages, it produces the full Iced element tree.
/// The caller caches this via `iced::widget::lazy` in the parent view.
pub fn view_download_progress(
    entry_index: usize,
    attachment: &DownloadAttachment,
    dark_mode: bool,
    overflow_open: bool,
    received_at_ms: Option<i64>,
    timeline_width: f32,
) -> iced::Element<'static, AppMessage> {
    #[cfg(feature = "video-playback")]
    {
        view_download_progress_inner(
            entry_index,
            attachment,
            dark_mode,
            overflow_open,
            None,
            false,
            None,
            false,
            received_at_ms,
            timeline_width,
        )
    }
    #[cfg(not(feature = "video-playback"))]
    {
        view_download_progress_inner(
            entry_index,
            attachment,
            dark_mode,
            overflow_open,
            (),
            false,
            received_at_ms,
            timeline_width,
        )
    }
}

#[cfg(feature = "video-playback")]
pub fn view_download_progress_with_player<'a>(
    entry_index: usize,
    attachment: &DownloadAttachment,
    dark_mode: bool,
    overflow_open: bool,
    player: Option<&'a Video>,
    preparing: bool,
    seek_position: Option<f32>,
    expanded: bool,
    received_at_ms: Option<i64>,
    timeline_width: f32,
) -> iced::Element<'a, AppMessage> {
    view_download_progress_inner(
        entry_index,
        attachment,
        dark_mode,
        overflow_open,
        player,
        preparing,
        seek_position,
        expanded,
        received_at_ms,
        timeline_width,
    )
}

fn view_download_progress_inner<'a>(
    entry_index: usize,
    attachment: &DownloadAttachment,
    dark_mode: bool,
    overflow_open: bool,
    #[cfg(feature = "video-playback")] player: Option<&'a Video>,
    #[cfg(not(feature = "video-playback"))] _player: (),
    preparing: bool,
    #[cfg(feature = "video-playback")] seek_position: Option<f32>,
    #[cfg(feature = "video-playback")] expanded: bool,
    received_at_ms: Option<i64>,
    timeline_width: f32,
) -> iced::Element<'a, AppMessage> {
    // Video attachments render through the reusable BoruVideoFileCard
    // component (see video_file_card.rs); this function keeps handling the
    // generic image/file download card.
    if attachment.kind == super::app::TransferKind::Video {
        #[cfg(feature = "video-playback")]
        {
            return crate::video_file_card::BoruVideoFileCard::new(
                entry_index,
                dark_mode,
                overflow_open,
                player,
                preparing,
                seek_position,
                expanded,
                received_at_ms,
                timeline_width,
            )
            .view(attachment);
        }
        #[cfg(not(feature = "video-playback"))]
        {
            return crate::video_file_card::BoruVideoFileCard::new(
                entry_index,
                dark_mode,
                overflow_open,
                (),
                preparing,
                received_at_ms,
                timeline_width,
            )
            .view(attachment);
        }
    }

    let state = &attachment.state;
    let theme = resolve_theme(dark_mode);
    let tone = state_badge_color(state, &theme);
    let muted = text_system(&theme);
    let name_str = attachment.name.clone();
    let error_color = color_error(&theme);

    // ── Row 1: State badge + filename + total size ──────────────────────
    let size_text = match &state {
        DownloadState::Active {
            total: Some(total), ..
        } if *total > 0 => human_size(*total),
        DownloadState::Active { bytes, .. } => {
            format!("{} received", human_size(*bytes))
        }
        DownloadState::Completed {
            total_size: Some(total),
            ..
        } if *total > 0 => human_size(*total),
        DownloadState::Paused {
            bytes,
            total: Some(total),
        } if *total > 0 => {
            format!("{} / {}", human_size(*bytes), human_size(*total))
        }
        DownloadState::Paused { bytes, .. } => {
            format!("{} received", human_size(*bytes))
        }
        DownloadState::Shared { size: Some(s), .. } if *s > 0 => human_size(*s),
        _ => String::new(),
    };

    // ── Row 1: State badge + filename + total size ──────────────────────
    // The card header carries the central Papirus file-type icon beside the
    // filename (PAPIRUS-10): the icon answers "what type of file is this?",
    // the state badge answers "what is happening to it" — status stays
    // separate from the file-type icon.  The icon is informative and shows
    // the friendly type in a hover tooltip (PAPIRUS-15 point 7); the
    // filename remains the primary content label.
    let file_type_icon = if attachment.is_folder {
        directory_icon_element(&attachment.name, FileTypeIconSize::Card, &theme)
    } else {
        file_type_icon_element_with_tooltip(
            &attachment.name,
            None,
            None,
            FileTypeIconSize::Card,
            &theme,
        )
    };

    let title_row = Row::new()
        .push(file_type_icon)
        .push(state_badge(state, tone))
        .push(
            crate::fonts::type_role_text(
                crate::fonts::TypeRole::ButtonLabel,
                attachment.name.clone(),
            )
            .color(tone)
            .wrapping(Wrapping::Word)
            .width(Length::Fill),
        )
        .push(
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, size_text)
                .color(muted)
                .width(Length::Shrink),
        )
        .align_y(Alignment::Center)
        .spacing(SPACE_8);

    // ── Row 2: Source peer (speed lives in the progress detail row) ────
    let source_row = {
        let source_label = if attachment.source_peer.is_empty() {
            String::new()
        } else {
            format!("From: {}", attachment.source_peer)
        };

        if source_label.is_empty() {
            None
        } else {
            Some(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, source_label)
                    .color(muted)
                    .width(Length::Fill),
            )
        }
    };

    // ── Row 2b: folder entry count (SENDME-01) ──────────────────────────
    let folder_info_row = if attachment.is_folder && attachment.collection_entries > 0 {
        Some(
            crate::fonts::type_role_text(
                crate::fonts::TypeRole::Metadata,
                format!("{} files in folder", attachment.collection_entries),
            )
            .color(muted)
            .width(Length::Fill),
        )
    } else {
        None
    };

    // ── Row 3: Progress bar + percentage ────────────────────────────────
    let progress_row = progress_section(state, dark_mode);

    // ── Row 3b: Bytes / total / percentage / speed detail line ─────────
    // Real transfer data only: bytes of total, percentage and transfer
    // speed are included only when the transfer layer provides them; no
    // invented estimates (VIDCARD-14).
    let progress_detail_row = active_download_detail(attachment).map(|detail| {
        crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, detail)
            .color(accent_green(&theme))
    });

    // ── Row 4: Action buttons ───────────────────────────────────────────
    let action_row = action_buttons(entry_index, attachment.kind, state, &name_str);
    let playback_action_row: Option<iced::Element<'a, AppMessage>> =
        attachment.playback_error.as_ref().and_then(|error| {
            error.retry_available().then(|| {
                secondary_button(None, "Retry player", AppMessage::PlayInlineVideo(entry_index))
                    .into()
            })
        });

    // ── Row 5: Failure reason (only in Failed state) ────────────────────
    let error_row = match &state {
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

            Some(column)
        }
        _ => None,
    };

    // ── Assemble the card ───────────────────────────────────────────────
    let mut body = Column::new().push(title_row).spacing(SPACE_6);

    if let Some(src) = source_row {
        body = body.push(src);
    }
    if let Some(folder_info) = folder_info_row {
        body = body.push(folder_info);
    }
    if let Some(prog) = progress_row {
        body = body.push(prog);
    }
    if let Some(progress_detail) = progress_detail_row {
        body = body.push(progress_detail);
    }
    body = body.push(action_row);
    if let Some(playback_actions) = playback_action_row {
        body = body.push(playback_actions);
    }
    // VIDCARD-13: "Open Folder" is now a light-bordered secondary action in
    // the completed/shared action row (see action_buttons); the old
    // default-styled blue "Open downloads folder" button is removed.
    if let Some(err) = error_row {
        // Extra visual separation for the error row
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
    body = body.spacing(SPACE_6);

    // Card container with state-coloured border.
    let card = container(body)
        .width(Length::Shrink)
        .padding([SPACE_12, SPACE_16])
        .style(move |t| widget::container::Style {
            background: Some(iced::Background::Color(bg_surface(t))),
            border: iced::Border {
                color: tone,
                width: 1.0,
                radius: SPACE_10.into(),
            },
            ..Default::default()
        });
    card.into()
}

// ── Sub-components ───────────────────────────────────────────────────────

/// Format a transfer speed in bytes/sec to a compact string like "2.1 MiB/s".
pub(crate) fn human_speed(bytes_per_sec: u64) -> String {
    format!("{}/s", human_size(bytes_per_sec))
}

/// Detail line for an in-flight download, e.g. `"18.2 MiB of 42.6 MiB •
/// 43% • 2.1 MiB/s"` (VIDCARD-14 spec: `"18.2 MB of 42.6 MB • 43% •
/// 2.1 MB/s"`).
///
/// Only real transfer-layer data is shown: bytes received always; total
/// size, percentage and transfer speed only when the transfer layer
/// actually provides them.  No estimated-time or other invented values.
/// Returns `None` for states that are not downloading.
pub(crate) fn active_download_detail(attachment: &DownloadAttachment) -> Option<String> {
    match &attachment.state {
        DownloadState::Active { bytes, total } | DownloadState::Paused { bytes, total } => {
            let mut parts = Vec::with_capacity(3);
            match total {
                Some(total) if *total > 0 => {
                    parts.push(format!(
                        "{} of {}",
                        human_size(*bytes),
                        human_size(*total)
                    ));
                    let pct = ((*bytes as f32 / *total as f32) * 100.0).round() as u8;
                    parts.push(format!("{pct}%"));
                }
                _ => parts.push(format!("{} received", human_size(*bytes))),
            }
            if let Some(speed) = attachment.speed_bytes_per_sec {
                parts.push(human_speed(speed));
            }
            Some(parts.join(" • "))
        }
        _ => None,
    }
}

/// Build the progress bar section: bar + percentage label.
///
/// VIDCARD-14: thin rounded track (pill), green fill in both light and
/// dark themes, smooth value updates from the real transfer state, and a
/// fixed-width percentage label so the bar row never re-measures while the
/// value climbs 0% → 100%.  The percentage is rendered as real text so the
/// progress value is accessible even though iced 0.14 exposes no widget
/// aria API.
pub(crate) fn progress_section<'a>(
    state: &DownloadState,
    dark_mode: bool,
) -> Option<iced::Element<'a, AppMessage>> {
    let (fraction, dimmed) = match state {
        DownloadState::Active {
            bytes,
            total: Some(total),
        } if *total > 0 => {
            let f = (*bytes as f32 / *total as f32).clamp(0.0, 1.0);
            (Some(f), false)
        }
        DownloadState::Paused {
            bytes,
            total: Some(total),
        } if *total > 0 => {
            let f = (*bytes as f32 / *total as f32).clamp(0.0, 1.0);
            (Some(f), true)
        }
        // Show an indeterminate label when downloading without a known total
        // (Phase 1: blob download to local store).  The progress bar stays
        // hidden but the user sees bytes received so they know the transfer
        // is active.
        DownloadState::Active { bytes, .. } if *bytes > 0 => (None, false),
        _ => return None,
    };

    let theme = resolve_theme(dark_mode);

    if let Some(fraction) = fraction {
        let pct = (fraction * 100.0).round() as u8;
        let bar = iced::widget::progress_bar(0.0..=1.0, fraction)
            .length(Length::Fill)
            .girth(Length::Fixed(PROGRESS_BAR_GIRTH))
            .style(move |t| {
                let (active, back) = if dimmed {
                    let c = border_muted(t);
                    (c, Color::from_rgba(c.r, c.g, c.b, 0.3))
                } else {
                    // Green fill in both themes (accent_primary turns blue
                    // in dark mode, so the spec's green bar uses the
                    // success green instead).
                    (accent_green(t), {
                        let c = border_muted(t);
                        Color::from_rgba(c.r, c.g, c.b, 0.4)
                    })
                };
                widget::progress_bar::Style {
                    background: back.into(),
                    bar: active.into(),
                    // Rounded track: the fill quad inherits this radius
                    // (with a transparent border), so a half-girth radius
                    // produces a modern thin pill in both track and fill.
                    border: iced::Border {
                        radius: (PROGRESS_BAR_GIRTH / 2.0).into(),
                        ..Default::default()
                    },
                }
            });

        let pct_label =
            crate::fonts::type_role_text(crate::fonts::TypeRole::BodyEmphasised, format!("{pct}%"))
                .color(if dimmed {
                    border_muted(&theme)
                } else {
                    accent_green(&theme)
                })
                // Fixed width keeps the row's layout stable as the value
                // climbs from 0% to 100% (no rapid layout changes).
                .width(Length::Fixed(PROGRESS_PCT_LABEL_WIDTH))
                .align_x(Alignment::End);

        Some(
            Row::new()
                .push(bar)
                .push(pct_label)
                .align_y(Alignment::Center)
                .spacing(SPACE_8)
                .into(),
        )
    } else {
        // No total known yet — show a simple bytes-received label
        if let DownloadState::Active { bytes, .. } = state {
            Some(
                Row::new()
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            format!("{} received — detecting size…", human_size(*bytes)),
                        )
                        .color(accent_green(&theme)),
                    )
                    .align_y(Alignment::Center)
                    .into(),
            )
        } else {
            None
        }
    }
}

/// Build the action-button row according to the current state.
///
/// VIDCARD-13: actions are state-appropriate with a green filled primary
/// button and light bordered secondary buttons.  The old default-styled
/// blue "Open downloads folder" button is replaced by a proper secondary
/// "Open Folder" action in the completed / shared states.
pub(crate) fn action_buttons<'a>(
    entry_index: usize,
    kind: super::app::TransferKind,
    state: &DownloadState,
    name: &str,
) -> iced::Element<'a, AppMessage> {
    use AppMessage::*;
    use super::app::TransferKind::Video;

    let buttons: Vec<iced::Element<'a, AppMessage>> = match (kind, state) {
        // ── Video: verifying (download complete, save pending) ──────────
        (Video, DownloadState::Completed { saved_path: None, .. }) => {
            vec![disabled_button("Verifying…").into()]
        }
        // ── Video: local file missing → re-download ─────────────────────
        (Video, DownloadState::Completed { saved_path: Some(path), .. }) if !path.exists() => {
            vec![primary_button(Some(ICON_RETRY), "Download", ExecuteDownloadAt(entry_index)).into()]
        }
        (Video, DownloadState::Failed { failure })
            if matches!(failure, super::app::DownloadFailure::FileRemoved) =>
        {
            vec![primary_button(Some(ICON_RETRY), "Download", ExecuteDownloadAt(entry_index)).into()]
        }
        // ── Video: download complete & playable ─────────────────────────
        (Video, DownloadState::Completed { saved_path: Some(path), .. }) if path.exists() => {
            vec![
                primary_button(Some(ICON_PLAY), "Play", PlayInlineVideo(entry_index)).into(),
                secondary_button(
                    Some(ICON_FILES),
                    "Open File",
                    OpenDownloadedFile(name.to_string()),
                )
                .into(),
                secondary_button(Some(ICON_FOLDER), "Open Folder", OpenDownloadsFolder).into(),
                secondary_button(Some(ICON_MESH), "Re-share", ReshareFile(entry_index)).into(),
            ]
        }
        // ── Video: outgoing shared file with a local copy ───────────────
        (Video, DownloadState::Shared { ref path, .. }) if path.exists() => {
            vec![
                primary_button(Some(ICON_PLAY), "Play", PlayInlineVideo(entry_index)).into(),
                secondary_button(
                    Some(ICON_FILES),
                    "Open File",
                    OpenDownloadedFile(name.to_string()),
                )
                .into(),
                secondary_button(Some(ICON_FOLDER), "Open Folder", OpenDownloadsFolder).into(),
                secondary_button(Some(ICON_MESH), "Re-share", ReshareFile(entry_index)).into(),
            ]
        }
        // ── Ready / not yet downloaded ──────────────────────────────────
        (_, DownloadState::Ready { .. }) => {
            vec![primary_button(Some(ICON_RETRY), "Download", ExecuteDownloadAt(entry_index)).into()]
        }
        // ── Download in progress: progress is the primary area; Cancel ──
        (_, DownloadState::Active { .. }) => {
            vec![
                secondary_button(None, "Pause", PauseDownloadAt(entry_index)).into(),
                text_button("Cancel", CancelDownloadAt(entry_index)).into(),
            ]
        }
        (_, DownloadState::Paused { .. }) => {
            vec![
                primary_button(Some(ICON_PLAY), "Resume", ResumeDownloadAt(entry_index)).into(),
                text_button("Cancel", CancelDownloadAt(entry_index)).into(),
            ]
        }
        // ── Generic completed / shared ──────────────────────────────────
        (_, DownloadState::Completed { .. }) => {
            vec![
                primary_button(Some(ICON_FILES), "Open", OpenDownloadedFile(name.to_string()))
                    .into(),
                secondary_button(Some(ICON_FOLDER), "Open Folder", OpenDownloadsFolder).into(),
                secondary_button(Some(ICON_COPY), "Copy Ticket", CopyShareTicket(entry_index))
                    .into(),
                secondary_button(Some(ICON_MESH), "Re-share", ReshareFile(entry_index)).into(),
            ]
        }
        (_, DownloadState::Shared { .. }) => {
            vec![
                primary_button(Some(ICON_FILES), "Open", OpenDownloadedFile(name.to_string()))
                    .into(),
                secondary_button(Some(ICON_FOLDER), "Open Folder", OpenDownloadsFolder).into(),
                secondary_button(Some(ICON_COPY), "Copy Ticket", CopyShareTicket(entry_index))
                    .into(),
                secondary_button(Some(ICON_MESH), "Re-share", ReshareFile(entry_index)).into(),
            ]
        }
        // ── Failed: Retry primary, Remove secondary ─────────────────────
        (_, DownloadState::Failed { failure }) if failure.retry_available() => {
            vec![
                primary_button(Some(ICON_RETRY), "Retry", ExecuteDownloadAt(entry_index)).into(),
                text_button("Remove", CancelDownloadAt(entry_index)).into(),
            ]
        }
        (_, DownloadState::Failed { .. }) => {
            vec![text_button("Remove", CancelDownloadAt(entry_index)).into()]
        }
        (_, DownloadState::Cancelled) => {
            vec![
                primary_button(Some(ICON_RETRY), "Retry", ExecuteDownloadAt(entry_index)).into(),
                text_button("Remove", CancelDownloadAt(entry_index)).into(),
            ]
        }
    };

    // Task 15: a wrapping row keeps the actions on one line at wide/medium
    // widths and lets them flow onto additional lines at narrow widths, so
    // the buttons never overflow the chat column horizontally.
    Row::with_children(buttons).spacing(SPACE_8).wrap().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{DownloadAttachment, DownloadState, TransferKind};

    fn attachment() -> DownloadAttachment {
        DownloadAttachment::new(TransferKind::Video, "clip.mp4", "ticket", "Duke", None)
    }

    #[test]
    fn active_detail_lists_bytes_total_percent_and_speed() {
        let mut att = attachment();
        att.state = DownloadState::Active {
            bytes: 19_000_000,
            total: Some(44_000_000),
        };
        att.speed_bytes_per_sec = Some(2_200_000);
        let detail = active_download_detail(&att).expect("active download has detail");
        // "18.1 MiB of 42.0 MiB • 43% • 2.1 MiB/s" — bytes of total, real
        // percentage, and the transfer-layer speed, in the spec's shape.
        assert!(detail.contains("of"), "expected 'of', got: {detail}");
        assert!(detail.contains("43%"), "expected 43%, got: {detail}");
        assert!(detail.contains("/s"), "expected speed, got: {detail}");
    }

    #[test]
    fn active_detail_omits_percent_when_total_unknown() {
        let mut att = attachment();
        att.state = DownloadState::Active {
            bytes: 5_000_000,
            total: None,
        };
        let detail = active_download_detail(&att).expect("active download has detail");
        assert!(detail.contains("received"), "got: {detail}");
        assert!(!detail.contains('%'), "percent must not be invented: {detail}");
    }

    #[test]
    fn active_detail_omits_speed_when_layer_does_not_provide_it() {
        let mut att = attachment();
        att.state = DownloadState::Active {
            bytes: 5_000_000,
            total: Some(10_000_000),
        };
        att.speed_bytes_per_sec = None;
        let detail = active_download_detail(&att).expect("active download has detail");
        assert!(detail.contains("50%"), "got: {detail}");
        assert!(!detail.contains('/'), "speed must not be invented: {detail}");
    }

    #[test]
    fn paused_snapshot_still_shows_real_progress_data() {
        let mut att = attachment();
        att.state = DownloadState::Paused {
            bytes: 10_000_000,
            total: Some(20_000_000),
        };
        let detail = active_download_detail(&att).expect("paused download has detail");
        assert!(detail.contains("50%"), "got: {detail}");
    }

    #[test]
    fn non_downloading_states_have_no_detail_line() {
        let att = attachment(); // Ready
        assert_eq!(active_download_detail(&att), None);
        let mut att = attachment();
        att.state = DownloadState::Completed {
            saved_name: "clip.mp4".into(),
            saved_path: None,
            total_size: Some(100),
        };
        assert_eq!(active_download_detail(&att), None);
    }

    // ── PAPIRUS-10: file-type icon element ─────────────────────────────

    #[test]
    fn file_type_icon_element_builds_for_each_semantic_size() {
        for size in [
            FileTypeIconSize::Compact,
            FileTypeIconSize::List,
            FileTypeIconSize::Card,
            FileTypeIconSize::Large,
            FileTypeIconSize::Hero,
        ] {
            // NOTE: choose a filename that resolves to an SVG asset path
            // distinct from the component's own handle-cache test path
            // (application-pdf.svg), so the two tests do not race on the
            // shared process-global SVG handle cache.
            let el: iced::Element<'_, AppMessage> =
                file_type_icon_element("report.docx", None, None, size, &iced::Theme::Light);
            let _ = el;
        }
    }

    #[test]
    fn file_type_icon_element_resolves_by_extension() {
        let el: iced::Element<'_, AppMessage> = file_type_icon_element(
            "photo.png",
            None,
            None,
            FileTypeIconSize::List,
            &iced::Theme::Light,
        );
        let _ = el;
    }

    #[test]
    fn file_type_icon_element_uses_advertised_mime_hint() {
        // application/zip resolves to an archive icon, distinct from the
        // component's own handle-cache test path (application-pdf.svg).
        let el: iced::Element<'_, AppMessage> = file_type_icon_element(
            "download.bin",
            Some("application/zip"),
            None,
            FileTypeIconSize::Card,
            &iced::Theme::Dark,
        );
        let _ = el;
    }

    #[test]
    fn file_type_icon_element_unknown_name_still_builds() {
        // The resolver's never-missing fallback chain must apply through the
        // chat helper too — an extensionless name never produces a broken
        // element.
        let el: iced::Element<'_, AppMessage> = file_type_icon_element(
            "unknownfile",
            None,
            None,
            FileTypeIconSize::Card,
            &iced::Theme::Light,
        );
        let _ = el;
    }

    #[test]
    fn file_type_icon_element_cache_is_deduped_by_key() {
        // Robust to test parallelism (other tests insert distinct keys into
        // the shared process-global cache concurrently): measure the delta
        // contributed by THIS key instead of assuming the map is empty.
        const KEY: &str = "cache-key-example-unique.docx";
        let cache = FILE_TYPE_ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let key_count =
            |needle: &str| usize::from(cache.lock().unwrap().keys().any(|k| k.filename == needle));
        // Count how many entries exist for this exact key (should be 0 or 1).
        let before = key_count(KEY);
        let _el: iced::Element<'_, AppMessage> =
            file_type_icon_element(KEY, None, None, FileTypeIconSize::List, &iced::Theme::Light);
        let _el2: iced::Element<'_, AppMessage> =
            file_type_icon_element(KEY, None, None, FileTypeIconSize::List, &iced::Theme::Light);
        // Two requests for the same key must not create two cache entries.
        assert_eq!(key_count(KEY), before + 1);
    }

    // ── PAPIRUS-15: accessibility entry points ─────────────────────────

    #[test]
    fn decorative_and_informative_icons_are_distinct_cache_entries() {
        // The decorative flag is part of the cache key: an informative and
        // a decorative rendering of the same file must not share a cached
        // configuration, otherwise a decorative caller could receive an
        // informative icon (with an accessible name) or vice versa.
        const KEY: &str = "decorative-cache-distinct-example.pdf";
        let cache = FILE_TYPE_ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let entry_for = |decorative: bool| {
            cache
                .lock()
                .unwrap()
                .iter()
                .find(|(key, _)| key.filename == KEY && key.decorative == decorative)
                .map(|(_, icon)| icon)
                .copied()
        };
        let _info: iced::Element<'_, AppMessage> =
            file_type_icon_element(KEY, None, None, FileTypeIconSize::List, &iced::Theme::Light);
        let _deco: iced::Element<'_, AppMessage> = decorative_file_type_icon_element(
            KEY,
            None,
            None,
            FileTypeIconSize::List,
            &iced::Theme::Light,
        );
        let informative = entry_for(false).expect("informative entry must exist");
        let decorative = entry_for(true).expect("decorative entry must exist");
        assert!(!informative.is_decorative());
        assert!(decorative.is_decorative());
        assert!(informative.effective_accessibility_label().is_some());
        assert_eq!(decorative.effective_accessibility_label(), None);
    }

    #[test]
    fn tooltip_and_plain_icons_are_distinct_cache_entries() {
        // The show_tooltip flag is part of the cache key for the same
        // reason as decorative: a caller that opts into a tooltip must not
        // receive the plain (no tooltip) cached configuration.
        const KEY: &str = "tooltip-cache-distinct-example.mp4";
        let cache = FILE_TYPE_ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let entry_for = |show_tooltip: bool| {
            cache
                .lock()
                .unwrap()
                .iter()
                .find(|(key, _)| key.filename == KEY && key.show_tooltip == show_tooltip)
                .map(|(_, icon)| icon)
                .copied()
        };
        let _plain: iced::Element<'_, AppMessage> =
            file_type_icon_element(KEY, None, None, FileTypeIconSize::List, &iced::Theme::Light);
        let _tooltip: iced::Element<'_, AppMessage> = file_type_icon_element_with_tooltip(
            KEY,
            None,
            None,
            FileTypeIconSize::List,
            &iced::Theme::Light,
        );
        let plain = entry_for(false).expect("plain entry must exist");
        let tooltip = entry_for(true).expect("tooltip entry must exist");
        assert!(!plain.is_decorative());
        assert!(!tooltip.is_decorative());
        // Both are informative; the accessible description is present either
        // way (the tooltip is an additional hover affordance).
        assert!(plain.effective_accessibility_label().is_some());
        assert!(tooltip.effective_accessibility_label().is_some());
    }

    // ── PAPIRUS-12: folder icons ─────────────────────────────────────────

    #[test]
    fn directory_icon_element_resolves_to_papirus_folder_icon() {
        // A folder display must resolve through the central resolver's
        // priority-1 directory state to the bundled Papirus folder icon —
        // never a generic outline or a filename-derived guess.
        let _el: iced::Element<'_, AppMessage> =
            directory_icon_element("shared-folder", FileTypeIconSize::List, &iced::Theme::Light);

        let cache = FILE_TYPE_ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let cache = cache.lock().unwrap();
        let entry = cache
            .iter()
            .find(|(key, _)| key.filename == "shared-folder" && key.is_directory)
            .map(|(_, icon)| icon)
            .expect("directory icon element must populate the shared cache");
        assert_eq!(entry.resolved().icon_id, "folder-open");
        assert_eq!(
            entry.resolved().file_category,
            crate::file_category::FileCategory::Folder
        );
        assert_eq!(
            entry.resolved().source,
            crate::file_type_resolver::ResolutionSource::Directory
        );
    }

    #[test]
    fn directory_icon_element_never_infers_folder_from_filename() {
        // Task 12 rule: a folder is explicit model state. A filename ending
        // with "/" passed through the FILE entry point must NOT become a
        // folder — the file path keeps resolving as a file (unknown here)
        // and the directory flag stays false in the cache key.
        let _file_el: iced::Element<'_, AppMessage> = file_type_icon_element(
            "photos/",
            None,
            None,
            FileTypeIconSize::List,
            &iced::Theme::Light,
        );
        let _dir_el: iced::Element<'_, AppMessage> =
            directory_icon_element("photos", FileTypeIconSize::List, &iced::Theme::Light);

        let cache = FILE_TYPE_ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let cache = cache.lock().unwrap();
        let file_entry = cache
            .iter()
            .find(|(key, _)| key.filename == "photos/" && !key.is_directory)
            .map(|(_, icon)| icon)
            .expect("file entry for photos/ must exist");
        assert_ne!(file_entry.resolved().file_category, crate::file_category::FileCategory::Folder);
        assert_eq!(
            file_entry.resolved().source,
            crate::file_type_resolver::ResolutionSource::UnknownFallback
        );
        // The same display name as an explicit folder is a separate cache
        // entry with the directory flag set — no cache collision.
        let dir_entry = cache
            .iter()
            .find(|(key, _)| key.filename == "photos" && key.is_directory)
            .map(|(_, icon)| icon)
            .expect("directory entry for photos must exist");
        assert_eq!(dir_entry.resolved().icon_id, "folder-open");
        assert_eq!(
            dir_entry.resolved().source,
            crate::file_type_resolver::ResolutionSource::Directory
        );
    }

    #[test]
    fn directory_and_file_same_name_do_not_share_cache_entry() {
        // A folder named "report.pdf" and a file named "report.pdf" are
        // different displays and must resolve independently: the folder to
        // the Papirus folder icon, the file to the PDF icon.
        let _dir: iced::Element<'_, AppMessage> =
            directory_icon_element("report.pdf", FileTypeIconSize::Card, &iced::Theme::Light);
        let _file: iced::Element<'_, AppMessage> = file_type_icon_element(
            "report.pdf",
            None,
            None,
            FileTypeIconSize::Card,
            &iced::Theme::Light,
        );

        let cache = FILE_TYPE_ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let cache = cache.lock().unwrap();
        let dir_entry = cache
            .iter()
            .find(|(key, _)| key.filename == "report.pdf" && key.is_directory)
            .map(|(_, icon)| icon)
            .expect("directory entry must exist");
        assert_eq!(dir_entry.resolved().icon_id, "folder-open");
        assert_eq!(
            dir_entry.resolved().file_category,
            crate::file_category::FileCategory::Folder
        );
        let file_entry = cache
            .iter()
            .find(|(key, _)| key.filename == "report.pdf" && !key.is_directory)
            .map(|(_, icon)| icon)
            .expect("file entry must exist");
        assert_eq!(file_entry.resolved().icon_id, "application-pdf");
        assert_eq!(
            file_entry.resolved().file_category,
            crate::file_category::FileCategory::Pdf
        );
    }

    // ── PAPIRUS-19: UI integration — same file, same icon, everywhere ──

    /// Task 19 UI integration (shared-component level): every listed Boru
    /// file surface — Chat, Shared by Me, Shared with Me, Downloading,
    /// Downloaded, Peers Downloading from Me, Activity Log, Re-share dialog,
    /// Transfer notification — renders its file-type icon through the SAME
    /// central component/resolver.  Full GUI automation is impractical in the
    /// remote-build harness, so this test drives the exact entry point each
    /// surface calls (`file_type_icon_element`, `decorative_*`,
    /// `file_type_icon_element_with_tooltip`, `directory_icon_element`) with
    /// the same file and asserts every surface resolves to the SAME icon id
    /// and category.  Because all four entry points share one
    /// `FILE_TYPE_ICON_CACHE` and one resolver, an identical result across
    /// the surface signatures is the strongest shared-component guarantee.
    #[test]
    fn task19_same_file_shows_same_icon_across_all_surfaces() {
        const FILE: &str = "report.pdf";
        const MIME: &str = "application/pdf";
        let theme = &iced::Theme::Light;

        // Drive each surface's exact call signature.
        // - Chat file-card header (download_progress_view.rs:705) and
        //   video cards (video_file_card.rs:773) use the tooltip variant.
        let _chat_card: iced::Element<'_, AppMessage> = file_type_icon_element_with_tooltip(
            FILE,
            None,
            None,
            FileTypeIconSize::Card,
            theme,
        );
        // - Chat image-header / generic attachment placeholder uses the
        //   tooltip variant at List/Large sizes (app.rs:29473/29577).
        let _chat_list: iced::Element<'_, AppMessage> = file_type_icon_element_with_tooltip(
            FILE,
            None,
            None,
            FileTypeIconSize::List,
            theme,
        );
        // - Shared by Me rows (shared_by_me_table.rs:758/768/777) and
        //   Shared with Me rows (app.rs:32612) pass the known MIME and use
        //   the decorative variant (filename is already printed).
        let _shared_by_me: iced::Element<'_, AppMessage> = decorative_file_type_icon_element(
            FILE,
            Some(MIME),
            None,
            FileTypeIconSize::List,
            theme,
        );
        let _shared_with_me: iced::Element<'_, AppMessage> = decorative_file_type_icon_element(
            FILE,
            Some(MIME),
            None,
            FileTypeIconSize::List,
            theme,
        );
        // - Downloading rows (app.rs:34372) and Peers Downloading from Me
        //   (app.rs:33734) use the informative variant, extension-only.
        let _downloading: iced::Element<'_, AppMessage> =
            file_type_icon_element(FILE, None, None, FileTypeIconSize::List, theme);
        let _peers_downloading: iced::Element<'_, AppMessage> =
            file_type_icon_element(FILE, None, None, FileTypeIconSize::Compact, theme);
        // - Downloaded rows (app.rs:34070) use the decorative variant with
        //   the recorded MIME hint.
        let _downloaded: iced::Element<'_, AppMessage> = decorative_file_type_icon_element(
            FILE,
            Some(MIME),
            None,
            FileTypeIconSize::List,
            theme,
        );
        // - Activity Log rows (app.rs:33310) and the transfer history /
        //   re-share surfaces (app.rs:34935, video_file_card.rs:984) use the
        //   informative compact variant.
        let _activity: iced::Element<'_, AppMessage> =
            file_type_icon_element(FILE, None, None, FileTypeIconSize::Compact, theme);

        // Every surface signature above must have populated the shared cache
        // with an entry whose resolved icon is the SAME PDF icon.
        let cache = FILE_TYPE_ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let cache = cache.lock().unwrap();
        let mut seen: Vec<String> = Vec::new();
        for (key, icon) in cache.iter() {
            if key.filename == FILE && !key.is_directory {
                seen.push(icon.resolved().icon_id.clone());
                assert_eq!(
                    icon.resolved().icon_id, "application-pdf",
                    "surface key {key:?} must resolve to the PDF icon"
                );
                assert_eq!(
                    icon.resolved().file_category,
                    crate::file_category::FileCategory::Pdf,
                    "surface key {key:?} must keep the PDF category"
                );
                assert!(
                    icon.resolved().asset_path.ends_with("32/application-pdf.svg"),
                    "surface key {key:?} must point at the bundled PDF SVG"
                );
            }
        }
        assert!(
            !seen.is_empty(),
            "no surface populated the shared icon cache for {FILE}"
        );
    }

    /// Task 19 UI integration: the same file shown as a FOLDER and as a FILE
    /// never collide, and a folder named like a document still renders the
    /// folder icon on every folder surface (Shared folders, re-share
    /// summaries, transfer rows).
    #[test]
    fn task19_same_folder_name_shows_folder_icon_on_folder_surfaces() {
        const NAME: &str = "report.pdf"; // adversarial: folder named like a PDF
        let theme = &iced::Theme::Light;

        // Folder surfaces all route through `directory_icon_element`
        // (download_progress_view.rs:258) — the PAPIRUS-12 entry point that
        // shared-folder rows, folder transfer summaries, and folder
        // re-share summaries use.
        let _shared_folder: iced::Element<'_, AppMessage> =
            directory_icon_element(NAME, FileTypeIconSize::List, theme);
        let _folder_transfer: iced::Element<'_, AppMessage> =
            directory_icon_element(NAME, FileTypeIconSize::Card, theme);

        // The file surface (same name, different file) stays a PDF.
        let _file_row: iced::Element<'_, AppMessage> =
            file_type_icon_element(NAME, None, None, FileTypeIconSize::List, theme);

        let cache = FILE_TYPE_ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let cache = cache.lock().unwrap();
        let folder = cache
            .iter()
            .find(|(key, _)| key.filename == NAME && key.is_directory)
            .map(|(_, icon)| icon)
            .expect("folder surface must populate the cache");
        assert_eq!(folder.resolved().icon_id, "folder-open");
        assert_eq!(
            folder.resolved().file_category,
            crate::file_category::FileCategory::Folder
        );
        let file = cache
            .iter()
            .find(|(key, _)| key.filename == NAME && !key.is_directory)
            .map(|(_, icon)| icon)
            .expect("file surface must populate the cache");
        assert_eq!(file.resolved().icon_id, "application-pdf");
    }

    /// Task 19 UI integration: a second required example (spreadsheet)
    /// shows the same spreadsheet icon on every surface signature, proving
    /// the consistency is per-type, not a single-file coincidence.
    #[test]
    fn task19_spreadsheet_same_icon_across_surfaces() {
        const FILE: &str = "budget.xlsx";
        const MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
        let theme = &iced::Theme::Light;

        let _chat: iced::Element<'_, AppMessage> =
            file_type_icon_element_with_tooltip(FILE, None, None, FileTypeIconSize::Card, theme);
        let _dashboard: iced::Element<'_, AppMessage> = decorative_file_type_icon_element(
            FILE,
            Some(MIME),
            None,
            FileTypeIconSize::List,
            theme,
        );
        let _transfer: iced::Element<'_, AppMessage> =
            file_type_icon_element(FILE, None, None, FileTypeIconSize::Compact, theme);

        let cache = FILE_TYPE_ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let cache = cache.lock().unwrap();
        let mut matches = 0;
        for (key, icon) in cache.iter() {
            if key.filename == FILE && !key.is_directory {
                matches += 1;
                assert_eq!(
                    icon.resolved().icon_id,
                    "application-vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                    "surface key {key:?} must resolve to the spreadsheet icon"
                );
                assert_eq!(
                    icon.resolved().file_category,
                    crate::file_category::FileCategory::Spreadsheet
                );
            }
        }
        assert!(matches >= 3, "expected at least 3 surfaces, got {matches}");
    }
}
