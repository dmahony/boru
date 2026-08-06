//! Reusable `BoruVideoFileCard` component for video file messages.
//!
//! This module owns the rendering of a video-file card in the chat log.
//! It is deliberately decoupled from the generic download-progress card
//! (image/file attachments still render through
//! [`crate::download_progress_view`]).
//!
//! The card is structured in four sections, mirroring the VIDCARD spec:
//!
//! - **Header** — state badge, video icon, filename, format label.
//! - **Media frame** — bounded poster or the active inline player, a play
//!   overlay (only when ready), and the playback-error panel when a live
//!   player failed to open the file.
//! - **Status and metadata** — transfer/playback status, sender, size and
//!   speed (real values only; unavailable metadata is hidden).
//! - **Actions** — state-appropriate buttons (Download / Pause / Resume /
//!   Cancel / Retry / Open / Re-share / Remove) plus the existing
//!   "Open downloads folder" link.
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
use iced::widget::{self, button, container, row, text, Column, Row};
use iced::{Alignment, Color, Length};
#[cfg(feature = "video-playback")]
use iced_video_player::{Video, VideoPlayer};

use super::app::{
    accent_primary, bg_surface, border_muted, color_error, text_system, SPACE_10, SPACE_12,
    SPACE_16, SPACE_2, SPACE_4, SPACE_6, SPACE_8, TYPO_SM, TYPO_XS, TYPO_XXS,
};
use super::app::{
    icon_svg, AppMessage, DownloadAttachment, DownloadState, ICON_ACTIVITY, ICON_FILES,
};
use super::download_progress_view::{
    action_button, action_buttons, human_size, human_speed, progress_section, resolve_theme,
    state_badge, state_badge_color,
};

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

// ── Bounded media sizing ───────────────────────────────────────────────

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

#[cfg(feature = "video-playback")]
fn format_media_time(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// Uppercase file extension used as the compact format label (e.g. "MP4").
fn file_format_label(name: &str) -> Option<String> {
    std::path::Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_uppercase())
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
    #[cfg(feature = "video-playback")]
    player: Option<&'a Video>,
    preparing: bool,
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
        #[cfg(feature = "video-playback")] player: Option<&'a Video>,
        #[cfg(not(feature = "video-playback"))] _player: (),
        preparing: bool,
        #[cfg(feature = "video-playback")] seek_position: Option<f32>,
        #[cfg(feature = "video-playback")] expanded: bool,
    ) -> Self {
        Self {
            entry_index,
            dark_mode,
            #[cfg(feature = "video-playback")]
            player,
            preparing,
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

        let header = self.header(attachment, tone, muted);
        let media = self.media_frame(attachment, muted, error_color);
        let status = self.status_metadata(attachment, &theme, tone, muted);
        let actions = self.actions(attachment);
        let error_section = self.error_section(attachment, tone, muted, error_color);

        let mut body = Column::new()
            .push(header)
            .push(media)
            .push(status)
            .push(actions)
            .spacing(SPACE_6);
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
        body = body.spacing(SPACE_6);

        container(body)
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
            })
            .into()
    }

    // ── Header: badge + icon + filename + format label + size ────────

    fn header(
        &self,
        attachment: &DownloadAttachment,
        tone: Color,
        muted: Color,
    ) -> iced::Element<'a, AppMessage> {
        let state = &attachment.state;
        let attachment_icon = match attachment.kind {
            super::app::TransferKind::Image => ICON_ACTIVITY,
            super::app::TransferKind::Video => ICON_ACTIVITY,
            super::app::TransferKind::File => ICON_FILES,
        };

        let size_text = match state {
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

        let format_label = file_format_label(&attachment.name);

        let mut title_row = Row::new()
            .push(
                icon_svg(attachment_icon, TYPO_SM)
                    .style(move |_t, _s| iced::widget::svg::Style { color: Some(tone) }),
            )
            .push(state_badge(state, tone))
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    attachment.name.clone(),
                )
                .color(tone)
                .wrapping(Wrapping::Word)
                .width(Length::Fill),
            );

        if let Some(format) = format_label {
            title_row = title_row.push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, format)
                    .color(muted)
                    .width(Length::Shrink),
            );
        }

        title_row = title_row.push(
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, size_text)
                .color(muted)
                .width(Length::Shrink),
        );

        title_row.align_y(Alignment::Center).spacing(SPACE_8).into()
    }

    // ── Media frame: poster or player + play overlay + error panel ────

    fn media_frame(
        &self,
        attachment: &DownloadAttachment,
        muted: Color,
        error_color: Color,
    ) -> iced::Element<'a, AppMessage> {
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
                AppMessage::PlayInlineVideo(self.entry_index)
            }
            #[cfg(not(feature = "video-playback"))]
            {
                AppMessage::OpenDownloadedFile(attachment.name.clone())
            }
        };
        let play = button(text("▶").size(28.0).color(Color::WHITE))
            .on_press_maybe(
                (presentation == VideoPresentationState::Ready && !self.preparing)
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
        let preview: iced::Element<'a, AppMessage> = container(widget::stack![
            poster,
            error_preview.unwrap_or_else(|| {
                if presentation == VideoPresentationState::Ready {
                    container(play)
                        .center_x(Length::Fill)
                        .center_y(Length::Fill)
                } else {
                    container(iced::widget::Space::new().width(0.0).height(0.0))
                }
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
        })
        .into();

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
            } if *total > 0 => human_size(*total),
            _ => String::new(),
        };

        let status = if self.preparing {
            "Preparing video…".to_string()
        } else if let Some(player_status) = self.playback_status() {
            player_status
        } else {
            match presentation {
                VideoPresentationState::Ready => "Ready to play".to_string(),
                VideoPresentationState::Downloading => "Downloading video…".to_string(),
                VideoPresentationState::Verifying => "Verifying video…".to_string(),
                VideoPresentationState::Failed => "Download failed".to_string(),
                VideoPresentationState::Missing => {
                    "Local file missing · download again".to_string()
                }
                VideoPresentationState::Remote => "Static preview · download to play".to_string(),
            }
        };

        let mut column = Column::new().push(
            text(format!("{size_label} · {status}"))
                .size(TYPO_XXS)
                .color(muted),
        );

        let source_label = if attachment.source_peer.is_empty() {
            String::new()
        } else {
            format!("From: {}", attachment.source_peer)
        };
        let speed_label = match state {
            DownloadState::Active { .. } => attachment
                .speed_bytes_per_sec
                .map(human_speed)
                .unwrap_or_default(),
            _ => String::new(),
        };
        if !source_label.is_empty() || !speed_label.is_empty() {
            column = column.push(
                Row::new()
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            source_label,
                        )
                        .color(muted)
                        .width(Length::Fill),
                    )
                    .push(
                        crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, speed_label)
                            .color(tone),
                    )
                    .align_y(Alignment::Center)
                    .spacing(SPACE_8),
            );
        }

        if let Some(prog) = progress_section(state, self.dark_mode) {
            column = column.push(prog);
        }
        if let DownloadState::Active { bytes, .. } = state {
            let detail = format!("{} received", human_size(*bytes));
            let speed = attachment
                .speed_bytes_per_sec
                .map(|s| format!(" • {}/s", human_size(s)))
                .unwrap_or_default();
            column = column.push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    format!("{detail}{speed}"),
                )
                .color(accent_primary(theme)),
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

        let action_row = action_buttons(self.entry_index, attachment.kind, state, &name_str);
        column = column.push(action_row);

        if let Some(error) = attachment.playback_error.as_ref() {
            if error.retry_available() {
                column = column.push(action_button(
                    "Retry player",
                    AppMessage::PlayInlineVideo(self.entry_index),
                ));
            }
        }

        // "Open folder" link — kept for behaviour parity; VIDCARD-13
        // replaces the default iced button styling.
        column = column.push(
            button(crate::fonts::type_role_text(
                crate::fonts::TypeRole::ButtonLabel,
                "Open downloads folder",
            ))
            .on_press(AppMessage::OpenDownloadsFolder)
            .padding([SPACE_2, SPACE_4]),
        );

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
        file_format_label, inline_video_preview_height, video_presentation_state,
        VideoPresentationState,
    };
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

    #[test]
    fn format_label_uses_real_extension_case_insensitively() {
        assert_eq!(file_format_label("clip.mp4"), Some("MP4".to_string()));
        assert_eq!(
            file_format_label("summer-trip.MOV"),
            Some("MOV".to_string())
        );
        assert_eq!(file_format_label("no_extension"), None);
    }
}
