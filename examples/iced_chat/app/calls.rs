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

    /// State-layer update for call screens (BORU-AUDIT-22 spec step 5).
    ///
    /// Handles every AppMessage variant owned by the calls feature: starting
    /// voice/video calls, call lifecycle events, accept/reject/hangup, mute/
    /// camera toggles, device selection and call command results. The root
    /// `update()` dispatches these variants here via a combined match arm.
    pub(crate) fn update_calls(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
            AppMessage::StartVoiceCall(peer) => {
                self.call_return_screen = Some(self.screen.clone());
                self.outgoing_call_peer = Some(peer);
                self.call_kind = Some(CallKind::Voice);
                self.call_was_incoming = false;
                self.call_declined = false;
                self.outgoing_call_status = Some(OutgoingCallStatus::Ringing);
                self.screen = Screen::OutgoingCall;
                let handle = self.call_handle.clone();
                iced::Task::perform(
                    async move { handle.start_voice_call(peer).await.map_err(|e| e.to_string()) },
                    AppMessage::CallStarted,
                )
            }
            AppMessage::StartVideoCall(peer) => {
                self.call_return_screen = Some(self.screen.clone());
                self.outgoing_call_peer = Some(peer);
                self.call_kind = Some(CallKind::Video);
                self.call_was_incoming = false;
                self.call_declined = false;
                self.outgoing_call_status = Some(OutgoingCallStatus::Ringing);
                self.screen = Screen::OutgoingCall;
                let handle = self.call_handle.clone();
                iced::Task::perform(
                    async move { handle.start_video_call(peer).await.map_err(|e| e.to_string()) },
                    AppMessage::CallStarted,
                )
            }
            AppMessage::CallStarted(result) => {
                match result {
                    Ok(call_id) => self.active_call_id = Some(call_id),
                    Err(error) => {
                        tracing::warn!(error = %error, "call start failed");
                        self.outgoing_call_status = Some(OutgoingCallStatus::Failed);
                        self.toast_message = Some(friendly_call_error_text(&error).to_string());
                    }
                }
                iced::Task::none()
            }
                        AppMessage::CallEventReceived(event) => {
                match &event {
                    CallEvent::Incoming { call_id, peer, kind } => {
                        self.active_call_id = Some(*call_id);
                        self.outgoing_call_peer = Some(*peer);
                        self.call_kind = Some(*kind);
                        self.call_was_incoming = true;
                        self.call_declined = false;
                        self.incoming_call = Some(IncomingCall { call_id: *call_id, peer: *peer, kind: *kind });
                        self.emit_incoming_call_notification(peer);
                    }
                    CallEvent::OutgoingRinging { peer, .. } => {
                        self.outgoing_call_peer = Some(*peer);
                        self.outgoing_call_status = Some(OutgoingCallStatus::Ringing);
                        self.screen = Screen::OutgoingCall;
                    }
                    CallEvent::Connecting { call_id } => self.active_call_id = Some(*call_id),
                    CallEvent::Active { call_id, peer, .. } => {
                        self.active_call_id = Some(*call_id);
                        self.outgoing_call_peer = Some(*peer);
                        self.call_was_incoming = self.incoming_call.as_ref().is_some_and(|call| call.call_id == *call_id);
                        self.call_started_at = Some(Instant::now());
                        self.screen = Screen::ActiveCall;
                        // The call is now in progress; the consent overlay is no longer needed.
                        if self.incoming_call.as_ref().is_some_and(|call| call.call_id == *call_id) {
                            self.incoming_call = None;
                        }
                    }
                    CallEvent::MediaStateChanged { call_id, audio_muted, video_enabled } => {
                        self.active_call_id = Some(*call_id);
                        self.call_audio_muted = *audio_muted;
                        self.call_camera_enabled = *video_enabled;
                    }
                    CallEvent::Ended { call_id, .. } => {
                        if self.active_call_id == Some(*call_id) {
                            if let CallEvent::Ended { reason, .. } = &event {
                                self.toast_message = Some(friendly_call_end(reason).to_string());
                            }
                            if let (Some(peer), Some(kind)) = (self.outgoing_call_peer, self.call_kind) {
                                let duration = self.call_started_at.map(|started| started.elapsed());
                                let outcome = if duration.is_some() {
                                    CallHistoryOutcome::Completed
                                } else if self.call_declined {
                                    CallHistoryOutcome::Declined
                                } else if self.call_was_incoming {
                                    CallHistoryOutcome::Missed
                                } else {
                                    CallHistoryOutcome::Failed
                                };
                                self.record_call_history(peer, kind, outcome, duration);
                            }
                            self.active_call_id = None;
                            self.outgoing_call_peer = None;
                            self.outgoing_call_status = None;
                            self.call_started_at = None;
                            self.call_kind = None;
                            self.call_was_incoming = false;
                            self.call_declined = false;
                            if let Some(screen) = self.call_return_screen.take() { self.screen = screen; }
                        }
                        if self.incoming_call.as_ref().is_some_and(|call| call.call_id == *call_id) {
                            self.incoming_call = None;
                        }
                    }
                    CallEvent::Failed { call_id, reason } => {
                        match call_id {
                            Some(cid) => {
                                if self.active_call_id == Some(*cid) {
                                    if matches!(reason, boru_core::call::manager::CallError::Rejected) {
                                        if let (Some(peer), Some(kind)) = (self.outgoing_call_peer, self.call_kind) {
                                            self.record_call_history(peer, kind, CallHistoryOutcome::Declined, None);
                                        }
                                    }
                                    self.active_call_id = None;
                                    self.outgoing_call_status = Some(match reason {
                                        boru_core::call::manager::CallError::Rejected => OutgoingCallStatus::Declined,
                                        boru_core::call::manager::CallError::Busy => OutgoingCallStatus::Busy,
                                        boru_core::call::manager::CallError::Connection => OutgoingCallStatus::Failed,
                                        _ => OutgoingCallStatus::Failed,
                                    });
                                    self.toast_message = Some(friendly_call_error(reason).to_string());
                                    self.call_kind = None;
                                    self.call_was_incoming = false;
                                    self.call_declined = false;
                                }
                                if self.incoming_call.as_ref().is_some_and(|call| call.call_id == *cid) {
                                    self.incoming_call = None;
                                }
                            }
                            None => { self.incoming_call = None; }
                        }
                    }
                    _ => {}
                }
                iced::Task::none()
            }
            AppMessage::AcceptIncomingCall(call_id) => {
                let handle = self.call_handle.clone();
                iced::Task::perform(async move { handle.accept(call_id).await.map_err(|e| e.to_string()) }, AppMessage::CallCommandFinished)
            }
            AppMessage::RejectIncomingCall(call_id) => {
                self.call_declined = true;
                let handle = self.call_handle.clone();
                iced::Task::perform(async move { handle.reject(call_id).await.map_err(|e| e.to_string()) }, AppMessage::CallCommandFinished)
            }
            AppMessage::HangUp(call_id) => {
                let handle = self.call_handle.clone();
                iced::Task::perform(async move { handle.hangup(call_id).await.map_err(|e| e.to_string()) }, AppMessage::CallCommandFinished)
            }
            AppMessage::ToggleCallMute => {
                if let Some(call_id) = self.active_call_id {
                    self.call_audio_muted = !self.call_audio_muted;
                    let handle = self.call_handle.clone();
                    let muted = self.call_audio_muted;
                    iced::Task::perform(async move { handle.set_muted(call_id, muted).await.map_err(|e| e.to_string()) }, AppMessage::CallCommandFinished)
                } else { iced::Task::none() }
            }
            AppMessage::ToggleCallCamera => {
                if let Some(call_id) = self.active_call_id {
                    self.call_camera_enabled = !self.call_camera_enabled;
                    let handle = self.call_handle.clone();
                    let enabled = self.call_camera_enabled;
                    iced::Task::perform(async move {
                        handle.set_camera_enabled(call_id, enabled).await.map_err(|e| e.to_string())
                    }, AppMessage::CallCommandFinished)
                } else { iced::Task::none() }
            }
            AppMessage::SelectCamera(selection) => {
                self.call_camera_selection = if selection == "next" {
                    if self.call_camera_selection == "Front camera" { "Back camera".to_string() } else { "Front camera".to_string() }
                } else { selection };
                iced::Task::none()
            }
            AppMessage::SelectMicrophone(_) | AppMessage::SelectSpeaker(_) | AppMessage::CallUiTick => iced::Task::none(),
            AppMessage::CallCommandFinished(Err(error)) => {
                tracing::warn!(error = %error, "call command failed");
                self.toast_message = Some(friendly_call_error_text(&error).to_string());
                iced::Task::none()
            }
            AppMessage::CallCommandFinished(Ok(())) => iced::Task::none(),
            // update() only dispatches the calls variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }
}
