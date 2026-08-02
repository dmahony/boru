//! Download progress widget — a stateless card rendering a single download row.
//!
//! This module provides [`view_download_progress`], a stateless widget that
//! renders a `DownloadAttachment` as a compact card with:
//!
//! - State badge (text + colour) indicating the current download status
//! - Filename and human-readable total size in the header row
//! - Source peer label and optional transfer speed
//! - Progress bar with percentage (for active/paused/verifying states)
//! - Context-sensitive action buttons (pause/resume/cancel/retry/open)
//! - Prominent failure reason in the Failed state
//!
//! All colors, spacing, and typography use the existing constants from the
//! parent module to stay consistent with the app's design system.

use iced::font::Weight;
use iced::widget::text::Wrapping;
use iced::widget::{self, button, container, row, text, Column, Row};
use iced::{Alignment, Color, Length};
#[cfg(feature = "video-playback")]
use iced_video_player::{Video, VideoPlayer};

use super::app::{
    icon_svg, AppMessage, DownloadAttachment, DownloadState, ICON_ACTIVITY, ICON_FILES,
};

// Re-import the design-token helpers and constants from app.rs.
use super::app::{
    accent_green, accent_primary, bg_surface, border_muted, color_error, text_system, SPACE_10,
    SPACE_12, SPACE_16, SPACE_2, SPACE_4, SPACE_6, SPACE_8, TYPO_SM, TYPO_XS, TYPO_XXS,
};

// ── Theme dispatch (light/dark) ──────────────────────────────────────────

/// Resolve the active Iced theme from the dark-mode flag.
fn resolve_theme(dark_mode: bool) -> iced::Theme {
    if dark_mode {
        iced::Theme::Dark
    } else {
        iced::Theme::Light
    }
}

/// Colour keyed to the current download state — used for the state badge.
fn state_badge_color(state: &DownloadState, theme: &iced::Theme) -> Color {
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
fn state_badge_label(state: &DownloadState) -> String {
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

// ── Human-readable byte formatting ───────────────────────────────────────

/// Format a byte count into a human-readable string (e.g., "4.2 MiB").
fn human_size(bytes: u64) -> String {
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

#[cfg(feature = "video-playback")]
fn format_media_time(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// Keep inline video previews bounded while retaining their known aspect ratio.
fn inline_video_preview_height(dimensions: Option<(u32, u32)>) -> f32 {
    let (width, height) = dimensions
        .filter(|(width, height)| *width > 0 && *height > 0)
        .map(|(width, height)| (width as f32, height as f32))
        .unwrap_or((16.0, 9.0));
    (360.0 / (width / height)).clamp(120.0, 280.0)
}

/// Compute the preview width from known poster dimensions, clamped sensibly.
fn inline_video_preview_width(dimensions: Option<(u32, u32)>) -> f32 {
    dimensions
        .filter(|(w, h)| *w > 0 && *h > 0)
        .map(|(w, _)| (w as f32).clamp(160.0, 640.0))
        .unwrap_or(360.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoPresentationState {
    Remote,
    Downloading,
    Verifying,
    Ready,
    Failed,
    Missing,
}

fn video_presentation_state(attachment: &DownloadAttachment) -> VideoPresentationState {
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
        DownloadState::Shared { ref path, .. } if path.exists() => {
            VideoPresentationState::Ready
        }
        DownloadState::Shared { .. } => VideoPresentationState::Missing,
        DownloadState::Failed { failure }
            if matches!(failure, super::app::DownloadFailure::FileRemoved) =>
        {
            VideoPresentationState::Missing
        }
        DownloadState::Failed { .. } => VideoPresentationState::Failed,
    }
}

// ── State badge pill ─────────────────────────────────────────────────────

fn state_badge(state: &DownloadState, tone: Color) -> iced::widget::Container<'static, AppMessage> {
    container(
        text(state_badge_label(state))
            .font(crate::fonts::inter(Weight::Semibold))
            .size(TYPO_XXS)
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
fn action_button<'a>(label: &'a str, msg: AppMessage) -> iced::widget::Button<'a, AppMessage> {
    let lbl = text(label)
        .font(crate::fonts::inter(Weight::Medium))
        .size(TYPO_XS);
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
fn text_button<'a>(label: &'a str, msg: AppMessage) -> iced::widget::Button<'a, AppMessage> {
    let lbl = text(label)
        .font(crate::fonts::inter(Weight::Normal))
        .size(TYPO_XS);
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
) -> iced::Element<'static, AppMessage> {
    #[cfg(feature = "video-playback")]
    {
        view_download_progress_inner(entry_index, attachment, dark_mode, None, false, None, false)
    }
    #[cfg(not(feature = "video-playback"))]
    {
        view_download_progress_inner(entry_index, attachment, dark_mode, (), false)
    }
}

#[cfg(feature = "video-playback")]
pub fn view_download_progress_with_player<'a>(
    entry_index: usize,
    attachment: &DownloadAttachment,
    dark_mode: bool,
    player: Option<&'a Video>,
    preparing: bool,
    seek_position: Option<f32>,
    expanded: bool,
) -> iced::Element<'a, AppMessage> {
    view_download_progress_inner(
        entry_index,
        attachment,
        dark_mode,
        player,
        preparing,
        seek_position,
        expanded,
    )
}

fn view_download_progress_inner<'a>(
    entry_index: usize,
    attachment: &DownloadAttachment,
    dark_mode: bool,
    #[cfg(feature = "video-playback")] player: Option<&'a Video>,
    #[cfg(not(feature = "video-playback"))] _player: (),
    preparing: bool,
    #[cfg(feature = "video-playback")] seek_position: Option<f32>,
    #[cfg(feature = "video-playback")] expanded: bool,
) -> iced::Element<'a, AppMessage> {
    let state = &attachment.state;
    let theme = resolve_theme(dark_mode);
    let tone = state_badge_color(state, &theme);
    let muted = text_system(&theme);
    let name_str = attachment.name.clone();
    let error_color = color_error(&theme);
    let attachment_icon = match attachment.kind {
        super::app::TransferKind::Image => ICON_ACTIVITY,
        super::app::TransferKind::Video => ICON_ACTIVITY,
        super::app::TransferKind::File => ICON_FILES,
    };

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
        DownloadState::Shared {
            size: Some(s), ..
        } if *s > 0 => human_size(*s),
        _ => String::new(),
    };

    let title_row = Row::new()
        .push(
            icon_svg(attachment_icon, TYPO_SM)
                .style(move |_t, _s| iced::widget::svg::Style { color: Some(tone) }),
        )
        .push(state_badge(state, tone))
        .push(
            text(attachment.name.clone())
                .font(crate::fonts::inter(Weight::Semibold))
                .size(TYPO_SM)
                .color(tone)
                .wrapping(Wrapping::Word)
                .width(Length::Fill),
        )
        .push(
            text(size_text)
                .font(crate::fonts::inter(Weight::Normal))
                .size(TYPO_XXS)
                .color(muted)
                .width(Length::Shrink),
        )
        .align_y(Alignment::Center)
        .spacing(SPACE_8);

    // ── Row 2: Source peer + speed ──────────────────────────────────────
    let source_row = {
        let source_label = if attachment.source_peer.is_empty() {
            String::new()
        } else {
            format!("From: {}", attachment.source_peer)
        };

        let speed_label = match &state {
            DownloadState::Active { .. } => attachment
                .speed_bytes_per_sec
                .map(human_speed)
                .unwrap_or_default(),
            _ => String::new(),
        };

        if source_label.is_empty() && speed_label.is_empty() {
            None
        } else {
            Some(
                Row::new()
                    .push(
                        text(source_label)
                            .font(crate::fonts::inter(Weight::Normal))
                            .size(TYPO_XS)
                            .color(muted)
                            .width(Length::Fill),
                    )
                    .push(
                        text(speed_label)
                            .font(crate::fonts::inter(Weight::Normal))
                            .size(TYPO_XS)
                            .color(tone),
                    )
                    .align_y(Alignment::Center)
                    .spacing(SPACE_8),
            )
        }
    };

    // ── Row 3: Progress bar + percentage ────────────────────────────────
    let progress_row = progress_section(state, dark_mode);

    // ── Row 3b: Speed + bytes detail (always visible when active) ─────
    let speed_detail_row = match state {
        DownloadState::Active { bytes, .. } => {
            let detail = format!("{} received", human_size(*bytes));
            let speed = attachment
                .speed_bytes_per_sec
                .map(|s| format!(" • {}/s", human_size(s)))
                .unwrap_or_default();
            Some(
                text(format!("{detail}{speed}"))
                    .font(crate::fonts::inter(Weight::Normal))
                    .size(TYPO_XS)
                    .color(accent_primary(&theme)),
            )
        }
        _ => None,
    };

    // ── Row 4: Action buttons ───────────────────────────────────────────
    let action_row = action_buttons(entry_index, attachment.kind, state, &name_str);
    let playback_action_row: Option<iced::Element<'a, AppMessage>> =
        attachment.playback_error.as_ref().and_then(|error| {
            error.retry_available().then(|| {
                action_button("Retry player", AppMessage::PlayInlineVideo(entry_index)).into()
            })
        });

    // ── Row 5: Failure reason (only in Failed state) ────────────────────
    let error_row = match &state {
        DownloadState::Failed { failure } => {
            let mut column = Column::new()
                .push(
                    row![
                        text(failure.title())
                            .font(crate::fonts::inter(Weight::Medium))
                            .size(TYPO_XS)
                            .color(error_color),
                        text(failure.stability_label())
                            .font(crate::fonts::inter(Weight::Normal))
                            .size(TYPO_XXS)
                            .color(tone),
                    ]
                    .spacing(SPACE_8)
                    .align_y(Alignment::Center),
                )
                .push(
                    text(failure.message())
                        .font(crate::fonts::inter(Weight::Normal))
                        .size(TYPO_XS)
                        .color(muted)
                        .width(Length::Fill),
                )
                .push(
                    text(format!("Recovery: {}", failure.recovery_action()))
                        .font(crate::fonts::inter(Weight::Normal))
                        .size(TYPO_XS)
                        .color(tone)
                        .width(Length::Fill),
                );

            if let Some(detail) = failure.diagnostics() {
                if !detail.is_empty() {
                    column = column.push(
                        text(detail)
                            .font(crate::fonts::jetbrains_mono(Weight::Normal))
                            .size(TYPO_XXS)
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

    // ── Static inline-video card ────────────────────────────────────────
    // Playback is intentionally deferred. The bounded poster and central
    // control establish the final footprint while the existing download,
    // retry, cancel, and open actions remain available below it.
    if attachment.kind == super::app::TransferKind::Video {
        let presentation = video_presentation_state(attachment);
        let preview_height = inline_video_preview_height(attachment.poster_dimensions);
        let preview_width = inline_video_preview_width(attachment.poster_dimensions);
        let poster: iced::Element<'static, AppMessage> =
            if let Some(ref handle) = attachment.thumbnail_handle {
                iced::widget::image(handle.clone())
                    .content_fit(iced::ContentFit::Cover)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            } else {
                container(
                    Column::new()
                        .push(text("VIDEO").size(TYPO_SM).color(muted))
                        .push(
                            text("Preview available after download")
                                .size(TYPO_XS)
                                .color(muted),
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
                AppMessage::PlayInlineVideo(entry_index)
            }
            #[cfg(not(feature = "video-playback"))]
            {
                AppMessage::OpenDownloadedFile(attachment.name.clone())
            }
        };
        let play = button(text("▶").size(28.0).color(Color::WHITE))
            .on_press_maybe(
                (presentation == VideoPresentationState::Ready && !preparing)
                    .then_some(play_message),
            )
            .padding([SPACE_8, SPACE_12])
            .style(|_theme, _status| widget::button::Style {
                background: Some(iced::Background::Color(Color::from_rgba(
                    0.0, 0.0, 0.0, 0.62,
                ))),
                border: iced::Border {
                    radius: 24.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            });
        let error_preview = attachment.playback_error.as_ref().map(|error| {
            container(
                Column::new()
                    .push(text(error.title()).size(TYPO_SM).color(error_color))
                    .push(text(error.message()).size(TYPO_XS).color(muted))
                    .push(
                        text("The original attachment is still available below.")
                            .size(TYPO_XXS)
                            .color(muted),
                    )
                    .spacing(SPACE_4)
                    .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
        });
        let preview = container(widget::stack![
            poster,
            error_preview.unwrap_or_else(|| {
                container(play)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
            })
        ])
        .width(Length::Fixed(preview_width))
        .height(Length::Fixed(preview_height))
        .clip(true)
        .style(|t| widget::container::Style {
            background: Some(iced::Background::Color(bg_surface(t))),
            border: iced::Border {
                color: border_muted(t),
                width: 1.0,
                radius: SPACE_10.into(),
            },
            ..Default::default()
        });
        #[cfg(feature = "video-playback")]
        let preview = if attachment.playback_error.is_some() {
            preview
        } else if let Some(video) = player {
            let duration = video.duration();
            let position = video.position().min(duration);
            let duration_secs = duration.as_secs_f32().max(f32::EPSILON);
            let fraction =
                seek_position.unwrap_or((position.as_secs_f32() / duration_secs).clamp(0.0, 1.0));
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
                            AppMessage::PlayInlineVideo(entry_index),
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
                            if expanded { "Collapse" } else { "Expand" },
                            AppMessage::InlineVideoToggleExpanded,
                        ))
                        .spacing(SPACE_6)
                        .align_y(Alignment::Center),
                );
            container(
                Column::new()
                    .push(
                        VideoPlayer::new(&video)
                            .content_fit(iced::ContentFit::Contain)
                            .on_end_of_stream(AppMessage::CloseInlineVideo)
                            .on_error(|_error| AppMessage::CloseInlineVideo),
                    )
                    .push(
                        container(controls)
                            .padding([SPACE_6, SPACE_8])
                            .style(|_theme| widget::container::Style {
                                background: Some(iced::Background::Color(Color::from_rgba(
                                    0.0, 0.0, 0.0, 0.76,
                                ))),
                                ..Default::default()
                            }),
                    ),
            )
            .width(Length::Shrink)
            .clip(true)
            .into()
        } else {
            preview
        };
        let size_label = match &attachment.state {
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
            } if *total > 0 => human_size(*total),
            _ => String::new(),
        };
        let status = if preparing {
            "Preparing video…"
        } else {
            match presentation {
                VideoPresentationState::Ready => "Ready to play",
                VideoPresentationState::Downloading => "Downloading video…",
                VideoPresentationState::Verifying => "Verifying video…",
                VideoPresentationState::Failed => "Download failed",
                VideoPresentationState::Missing => "Local file missing · download again",
                VideoPresentationState::Remote => "Static preview · download to play",
            }
        };
        body = body.push(preview).push(
            text(format!("{size_label} · {status}"))
                .size(TYPO_XXS)
                .color(muted),
        );
    }

    if let Some(src) = source_row {
        body = body.push(src);
    }
    if let Some(prog) = progress_row {
        body = body.push(prog);
    }
    if let Some(speed_detail) = speed_detail_row {
        body = body.push(speed_detail);
    }
    body = body.push(action_row);
    if let Some(playback_actions) = playback_action_row {
        body = body.push(playback_actions);
    }
    // "Open folder" link — always visible below the action buttons
    body = body.push(
        button(
            text("Open downloads folder")
                .font(crate::fonts::inter(Weight::Medium))
                .size(TYPO_XS),
        )
        .on_press(AppMessage::OpenDownloadsFolder)
        .padding([SPACE_2, SPACE_4]),
    );
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
fn human_speed(bytes_per_sec: u64) -> String {
    format!("{}/s", human_size(bytes_per_sec))
}

/// Build the progress bar section: bar + percentage label.
fn progress_section<'a>(
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
            .girth(Length::Fixed(6.0))
            .style(move |t| {
                let (active, back) = if dimmed {
                    let c = border_muted(t);
                    (c, Color::from_rgba(c.r, c.g, c.b, 0.3))
                } else {
                    (accent_primary(t), {
                        let c = border_muted(t);
                        Color::from_rgba(c.r, c.g, c.b, 0.4)
                    })
                };
                widget::progress_bar::Style {
                    background: back.into(),
                    bar: active.into(),
                    border: iced::Border::default(),
                }
            });

        let pct_label = text(format!("{pct}%"))
            .font(crate::fonts::inter(Weight::Bold))
            .size(TYPO_XXS)
            .color(if dimmed {
                border_muted(&theme)
            } else {
                accent_primary(&theme)
            });

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
                        text(format!("{} received — detecting size…", human_size(*bytes)))
                            .font(crate::fonts::inter(Weight::Normal))
                            .size(TYPO_XS)
                            .color(accent_primary(&theme)),
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
fn action_buttons<'a>(
    entry_index: usize,
    kind: super::app::TransferKind,
    state: &DownloadState,
    name: &str,
) -> iced::Element<'a, AppMessage> {
    use AppMessage::*;

    let buttons: Vec<iced::Element<'a, AppMessage>> = match (kind, state) {
        (
            super::app::TransferKind::Video,
            DownloadState::Completed {
                saved_path: None, ..
            },
        ) => {
            vec![text_button("Verifying…", AppMessage::Noop).into()]
        }
        (
            super::app::TransferKind::Video,
            DownloadState::Completed {
                saved_path: Some(path),
                ..
            },
        ) if !path.exists() => {
            vec![action_button("Download", ExecuteDownloadAt(entry_index)).into()]
        }
        (super::app::TransferKind::Video, DownloadState::Failed { failure })
            if matches!(failure, super::app::DownloadFailure::FileRemoved) =>
        {
            vec![action_button("Download", ExecuteDownloadAt(entry_index)).into()]
        }
        (_, DownloadState::Ready { .. }) => {
            vec![action_button("Download", ExecuteDownloadAt(entry_index)).into()]
        }
        (_, DownloadState::Active { .. }) => {
            vec![
                action_button("Pause", PauseDownloadAt(entry_index)).into(),
                text_button("Cancel", CancelDownloadAt(entry_index)).into(),
            ]
        }
        (_, DownloadState::Paused { .. }) => {
            vec![
                action_button("Resume", ResumeDownloadAt(entry_index)).into(),
                text_button("Cancel", CancelDownloadAt(entry_index)).into(),
            ]
        }
        (_, DownloadState::Completed { .. }) => {
            vec![
                action_button("Open", OpenDownloadedFile(name.to_string())).into(),
                text_button("Re-share", ReshareFile(entry_index)).into(),
            ]
        }
        (_, DownloadState::Shared { .. }) => {
            vec![
                action_button("Open", OpenDownloadedFile(name.to_string())).into(),
                text_button("Re-share", ReshareFile(entry_index)).into(),
            ]
        }
        (_, DownloadState::Failed { failure }) if failure.retry_available() => {
            vec![
                action_button("Retry", ExecuteDownloadAt(entry_index)).into(),
                text_button("Remove", CancelDownloadAt(entry_index)).into(),
            ]
        }
        (_, DownloadState::Failed { .. }) => {
            vec![text_button("Remove", CancelDownloadAt(entry_index)).into()]
        }
        (_, DownloadState::Cancelled) => {
            vec![
                action_button("Retry", ExecuteDownloadAt(entry_index)).into(),
                text_button("Remove", CancelDownloadAt(entry_index)).into(),
            ]
        }
    };

    Row::with_children(buttons).spacing(SPACE_8).into()
}

#[cfg(test)]
mod tests {
    use super::{inline_video_preview_height, video_presentation_state, VideoPresentationState};
    use crate::app::{DownloadAttachment, DownloadFailure, DownloadState, TransferKind};
    use std::path::PathBuf;

    #[test]
    fn unknown_aspect_ratio_uses_bounded_widescreen_default() {
        assert_eq!(inline_video_preview_height(None), 202.5);
    }

    #[test]
    fn portrait_and_landscape_previews_are_bounded() {
        assert_eq!(inline_video_preview_height(Some((100, 1000))), 280.0);
        assert_eq!(inline_video_preview_height(Some((3840, 2160))), 202.5);
        assert_eq!(inline_video_preview_height(Some((1000, 100))), 120.0);
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
}
