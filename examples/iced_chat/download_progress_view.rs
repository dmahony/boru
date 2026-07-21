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

use iced::widget::{self, button, container, row, text, Column, Row};
use iced::{Alignment, Color, Length};

use super::app::{AppMessage, DownloadAttachment, DownloadState};

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
        DownloadState::Ready | DownloadState::Active { .. } | DownloadState::Paused { .. } => {
            accent_primary(theme)
        }
        DownloadState::Completed { .. } => accent_green(theme),
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
        DownloadState::Ready => "Pending".to_string(),
        DownloadState::Active { .. } => "Downloading".to_string(),
        DownloadState::Paused { .. } => "Paused".to_string(),
        DownloadState::Completed { .. } => "Complete".to_string(),
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

// ── State badge pill ─────────────────────────────────────────────────────

fn state_badge(state: &DownloadState, tone: Color) -> iced::widget::Container<'static, AppMessage> {
    container(text(state_badge_label(state)).size(TYPO_XXS).color(
        // Use a perceptually balanced off-white against the badge color
        Color::from_rgb(0.95, 0.95, 0.95),
    ))
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
    let lbl = text(label).size(TYPO_XS);
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
    let lbl = text(label).size(TYPO_XS);
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
        _ => String::new(),
    };

    let title_row = Row::new()
        .push(state_badge(state, tone))
        .push(
            text(attachment.name.clone())
                .size(TYPO_SM)
                .color(tone)
                .width(Length::Fill),
        )
        .push(
            text(size_text)
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
                            .size(TYPO_XS)
                            .color(muted)
                            .width(Length::Fill),
                    )
                    .push(text(speed_label).size(TYPO_XS).color(tone))
                    .align_y(Alignment::Center)
                    .spacing(SPACE_8),
            )
        }
    };

    // ── Row 3: Progress bar + percentage ────────────────────────────────
    let progress_row = progress_section(state, dark_mode);

    // ── Row 4: Action buttons ───────────────────────────────────────────
    let action_row = action_buttons(entry_index, state, &name_str);

    // ── Row 5: Failure reason (only in Failed state) ────────────────────
    let error_row = match &state {
        DownloadState::Failed { failure } => {
            let mut column = Column::new()
                .push(
                    row![
                        text(failure.title()).size(TYPO_XS).color(error_color),
                        text(failure.stability_label()).size(TYPO_XXS).color(tone),
                    ]
                    .spacing(SPACE_8)
                    .align_y(Alignment::Center),
                )
                .push(
                    text(failure.message())
                        .size(TYPO_XS)
                        .color(muted)
                        .width(Length::Fill),
                )
                .push(
                    text(format!("Recovery: {}", failure.recovery_action()))
                        .size(TYPO_XS)
                        .color(tone)
                        .width(Length::Fill),
                );

            if let Some(detail) = failure.diagnostics() {
                if !detail.is_empty() {
                    column =
                        column.push(text(detail).size(TYPO_XXS).color(muted).width(Length::Fill));
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
    body = body.push(action_row);
    // "Open folder" link — always visible below the action buttons
    body = body.push(
        button(text("Open downloads folder").size(TYPO_XS))
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

    // Card container with state-coloured border
    let card = container(body)
        .width(Length::Fill)
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
            (f, false)
        }
        DownloadState::Paused {
            bytes,
            total: Some(total),
        } if *total > 0 => {
            let f = (*bytes as f32 / *total as f32).clamp(0.0, 1.0);
            (f, true)
        }
        _ => return None,
    };

    let pct = (fraction * 100.0).round() as u8;
    let theme = resolve_theme(dark_mode);
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

    let pct_label = text(format!("{pct}%")).size(TYPO_XXS).color(if dimmed {
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
}

/// Build the action-button row according to the current state.
fn action_buttons<'a>(
    entry_index: usize,
    state: &DownloadState,
    name: &str,
) -> iced::Element<'a, AppMessage> {
    use AppMessage::*;

    let buttons: Vec<iced::Element<'a, AppMessage>> = match state {
        DownloadState::Ready => {
            vec![action_button("Download", ExecuteDownloadAt(entry_index)).into()]
        }
        DownloadState::Active { .. } => {
            vec![
                action_button("Pause", PauseDownloadAt(entry_index)).into(),
                text_button("Cancel", CancelDownloadAt(entry_index)).into(),
            ]
        }
        DownloadState::Paused { .. } => {
            vec![
                action_button("Resume", ResumeDownloadAt(entry_index)).into(),
                text_button("Cancel", CancelDownloadAt(entry_index)).into(),
            ]
        }
        DownloadState::Completed { .. } => {
            vec![action_button("Open", OpenDownloadedFile(name.to_string())).into()]
        }
        DownloadState::Failed { failure } if failure.retry_available() => {
            vec![
                action_button("Retry", ExecuteDownloadAt(entry_index)).into(),
                text_button("Remove", CancelDownloadAt(entry_index)).into(),
            ]
        }
        DownloadState::Failed { .. } => {
            vec![text_button("Remove", CancelDownloadAt(entry_index)).into()]
        }
        DownloadState::Cancelled => {
            vec![
                action_button("Retry", ExecuteDownloadAt(entry_index)).into(),
                text_button("Remove", CancelDownloadAt(entry_index)).into(),
            ]
        }
    };

    Row::with_children(buttons).spacing(SPACE_8).into()
}
