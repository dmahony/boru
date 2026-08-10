//! Call screens (outgoing / active).
//!
//! Extracted from app.rs (BORU-AUDIT-22). Owns the outgoing-call and
//! active-call screen views: the `impl IcedChat` methods that build and
//! render them. Reads app state via `use super::*`; app.rs re-exports
//! the pub(crate) items it still references with `use calls::*`.

use super::*;

impl IcedChat {
    pub(crate) fn view_outgoing_call(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, text};
        use iced::{Alignment, Length};

        let peer = self.outgoing_call_peer;
        let name = peer
            .as_ref()
            .map(|key| self.resolve_name(key))
            .unwrap_or_else(|| "Unknown contact".to_string());
        let status = match self.outgoing_call_status {
            Some(OutgoingCallStatus::Ringing) => "Ringing…",
            Some(OutgoingCallStatus::Declined) => "Call declined",
            Some(OutgoingCallStatus::Busy) => "User is busy",
            Some(OutgoingCallStatus::Failed) => "Call failed",
            None => "Calling…",
        };
        let initials = crate::presentation::initials(&name);
        let avatar_label = if initials.is_empty() { "?".to_string() } else { initials };
        let avatar = container(text(avatar_label).size(36.0))
            .width(Length::Fixed(96.0))
            .height(Length::Fixed(96.0))
            .center_x(Length::Fixed(96.0))
            .center_y(Length::Fixed(96.0))
            .style(|theme| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg_surface_secondary(theme))),
                border: iced::Border { radius: 48.0.into(), ..Default::default() },
                ..Default::default()
            });
        let controls: iced::Element<'_, AppMessage> = match self.active_call_id {
            Some(call_id) => button(text("Cancel"))
                .on_press(AppMessage::HangUp(call_id))
                .padding([SPACE_8, SPACE_24])
                .style(BUTTON_DANGER)
                .into(),
            None => iced::widget::Space::new().height(Length::Fixed(40.0)).into(),
        };
        container(column![avatar, text(name).size(24.0), text(status).size(16.0), controls]
            .spacing(SPACE_16)
            .align_x(Alignment::Center))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    pub(crate) fn view_active_call(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, row, text};
        use iced::{Alignment, Length};

        let name = self.outgoing_call_peer.as_ref().map(|peer| self.resolve_name(peer)).unwrap_or_else(|| "Unknown contact".to_string());
        let initials = crate::presentation::initials(&name);
        let avatar_label = if initials.is_empty() { "?".to_string() } else { initials };
        let remote_fallback = || container(column![text(avatar_label.clone()).size(44.0), text(name.clone()).size(18.0)]
            .spacing(SPACE_8).align_x(Alignment::Center))
            .width(Length::Fill).height(Length::Fill)
            .center_x(Length::Fill).center_y(Length::Fill)
            .style(|theme| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg_surface_secondary(theme))),
                ..Default::default()
            });
        #[cfg(feature = "video-calls")]
        let remote = self
            .latest_remote_frame
            .as_ref()
            .filter(|frame| contain_fit_rect(frame.width as f32, frame.height as f32, 1.0, 1.0).is_some())
            .map(|frame| {
                // Iced performs the final dynamic viewport calculation;
                // Contain is the rendering equivalent of contain_fit_rect
                // and preserves the source ratio with letterboxing.
            iced::widget::image(iced::widget::image::Handle::from_rgba(
                frame.width, frame.height, frame.rgba.to_vec()))
                .content_fit(iced::ContentFit::Contain)
                .width(Length::Fill).height(Length::Fill).into()
            });
        #[cfg(not(feature = "video-calls"))]
        let remote: Option<iced::Element<'_, AppMessage>> = None;
        // The remote stage is the main area: show the latest remote frame
        // whenever one is available (remote camera on), and fall back to the
        // avatar/name block when the remote camera is off (no frame yet).
        // The LOCAL camera state must not gate the remote stage — turning off
        // your own camera only affects the local PiP.
        let remote_main: iced::Element<'_, AppMessage> = remote.unwrap_or_else(|| remote_fallback().into());
        #[cfg(feature = "video-calls")]
        let local = self.latest_local_frame.as_ref().and_then(|frame| {
            let fit = contain_fit_rect(frame.width as f32, frame.height as f32, 220.0, 150.0)?;
            Some(
            iced::widget::image(iced::widget::image::Handle::from_rgba(
                frame.width, frame.height, frame.rgba.to_vec()))
                .content_fit(iced::ContentFit::Contain)
                .width(Length::Fixed(fit.width)).height(Length::Fixed(fit.height)).into(),
            )
        });
        #[cfg(not(feature = "video-calls"))]
        let local: Option<iced::Element<'_, AppMessage>> = None;
        let local_pip: iced::Element<'_, AppMessage> = local.unwrap_or_else(|| container(text("You").size(18.0))
            .width(Length::Fixed(220.0)).height(Length::Fixed(150.0))
            .center_x(Length::Fixed(220.0)).center_y(Length::Fixed(150.0))
            .style(|theme| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg_surface_secondary(theme))),
                border: iced::Border { radius: 12.0.into(), ..Default::default() },
                ..Default::default()
            }).into());
        let elapsed = self.call_started_at.map(|start| start.elapsed().as_secs()).unwrap_or_default();
        let duration = format!("{:02}:{:02}", elapsed / 60, elapsed % 60);
        let status = if self.call_audio_muted { "Connected · Microphone muted" } else { "Connected · Audio" };
        let mute_label = if self.call_audio_muted { "Unmute" } else { "Mute" };
        let mute = button(text(mute_label)).on_press_maybe(self.active_call_id.map(|_| AppMessage::ToggleCallMute));
        let camera_label = if self.call_camera_enabled { "Camera Off" } else { "Camera On" };
        let camera = button(text(camera_label)).on_press_maybe(self.active_call_id.map(|_| AppMessage::ToggleCallCamera));
        let switch_camera = button(text(format!("Switch Camera · {}", self.call_camera_selection)))
            .on_press_maybe(self.active_call_id.map(|_| AppMessage::SelectCamera("next".to_string())));
        let hang_up = button(text("Hang Up"))
            .on_press_maybe(self.active_call_id.map(AppMessage::HangUp))
            .style(BUTTON_DANGER);
        let stage = container(local_pip)
            .width(Length::Fixed(220.0)).height(Length::Fixed(150.0))
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Bottom)
            .style(|theme| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg_surface_secondary(theme))),
                border: iced::Border { radius: 12.0.into(), ..Default::default() },
                ..Default::default()
            });
        container(column![
            container(remote_main).width(Length::Fill).height(Length::Fill),
            stage,
            text(name).size(26.0), text(duration).size(22.0), text(status).size(16.0),
            row![mute, camera, switch_camera, hang_up].spacing(SPACE_12)
        ].spacing(SPACE_12).align_x(Alignment::Center))
            .width(Length::Fill).height(Length::Fill)
            .padding(SPACE_16).into()
    }
}
