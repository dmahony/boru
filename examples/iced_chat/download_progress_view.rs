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
    icon_svg, AppMessage, DownloadAttachment, DownloadState, ICON_FILES, ICON_FOLDER, ICON_MESH,
    ICON_PLAY, ICON_RETRY,
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
}

impl std::hash::Hash for FileTypeIconKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.filename.hash(state);
        self.mime_type.hash(state);
        self.detected_type.hash(state);
        // FileTypeIconSize does not derive Hash; its Papirus size directory is
        // injective (16/24/32/48/64 map one-to-one), so hash that instead.
        self.size.papirus_dir().hash(state);
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
pub(crate) fn file_type_icon_element(
    filename: &str,
    mime_type: Option<&str>,
    detected_type: Option<&str>,
    size: FileTypeIconSize,
    theme: &iced::Theme,
) -> iced::Element<'static, AppMessage> {
    let key = FileTypeIconKey {
        filename: filename.to_string(),
        mime_type: mime_type.map(str::to_string),
        detected_type: detected_type.map(str::to_string),
        size,
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
    let icon: &'static FileTypeIcon<'static> = Box::leak(Box::new(
        FileTypeIcon::new(filename, mime_type, detected_type, false).size(key.size),
    ));
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
pub(crate) fn action_button<'a>(label: &'a str, msg: AppMessage) -> iced::widget::Button<'a, AppMessage> {
    let lbl = crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, label);
    button(lbl)
        .on_press(msg)
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
        })
}

/// A subtle text-only button (borderless, uses muted/destructive colour).
pub(crate) fn text_button<'a>(label: &'a str, msg: AppMessage) -> iced::widget::Button<'a, AppMessage> {
    let lbl = crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, label);
    button(lbl)
        .on_press(msg)
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
        })
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
pub(crate) fn primary_button<'a>(
    icon: Option<&'static [u8]>,
    label: &'a str,
    msg: AppMessage,
) -> iced::widget::Button<'a, AppMessage> {
    button(action_content(icon, label, |_t| Color::WHITE))
        .on_press(msg)
        .padding([SPACE_6, SPACE_12])
        .style(super::app::BUTTON_PRIMARY_GREEN)
}

/// Light bordered secondary action button (supporting actions per state).
pub(crate) fn secondary_button<'a>(
    icon: Option<&'static [u8]>,
    label: &'a str,
    msg: AppMessage,
) -> iced::widget::Button<'a, AppMessage> {
    button(action_content(icon, label, text_system))
        .on_press(msg)
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
        })
}

/// Disabled / loading button — no press handler, muted styling.
pub(crate) fn disabled_button<'a>(label: &'a str) -> iced::widget::Button<'a, AppMessage> {
    let lbl = crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, label);
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
        })
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
    // separate from the file-type icon.
    let file_type_icon =
        file_type_icon_element(&attachment.name, None, None, FileTypeIconSize::Card, &theme);

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
                secondary_button(Some(ICON_MESH), "Re-share", ReshareFile(entry_index)).into(),
            ]
        }
        (_, DownloadState::Shared { .. }) => {
            vec![
                primary_button(Some(ICON_FILES), "Open", OpenDownloadedFile(name.to_string()))
                    .into(),
                secondary_button(Some(ICON_FOLDER), "Open Folder", OpenDownloadsFolder).into(),
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
            let el: iced::Element<'_, AppMessage> =
                file_type_icon_element("report.pdf", None, None, size, &iced::Theme::Light);
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
        let el: iced::Element<'_, AppMessage> = file_type_icon_element(
            "download.bin",
            Some("application/pdf"),
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
}
