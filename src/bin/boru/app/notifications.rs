//! Notifications & activity domain (BORU-APP-004).
//!
//! Extracted from app.rs. Owns the notification service + window focus
//! tracker (desktop/OS notifications), the in-app toast surface, and the
//! landing-page Recent Activity feed (ring buffer + per-second tick).
//!
//! ## Pattern
//!
//! [`NotificationsState`] is the DomainState: `IcedChat` holds exactly one
//! instance (`self.notifications_state`) and there is no mirror of this
//! state anywhere else in the app (PDF §14 "same state in both modules"
//! stop condition). [`NotificationsMessage`] is the DomainMessage enum;
//! the shell routes the matching `AppMessage` variants to
//! [`NotificationsState::update`]. The three domain messages have no shell
//! side effects, so `update()` returns nothing — cross-domain effects that
//! appear later must be returned as typed events per `domain_pattern.md`.
//!
//! The notification *emit* helpers (`emit_message_notification`,
//! `emit_incoming_call_notification`) remain `impl IcedChat` methods hosted
//! here because they need read-only shell context (`conversation_store`,
//! `resolve_name`) to build their events — same convention as the heavier
//! settings arms in `app/settings.rs`. They mutate only this domain's
//! state (`notification_service`, `window_focus_tracker`).
//!
//! The toast surface is written by many other domains (chat, files,
//! tunnels, calls, contacts) through the typed [`NotificationsState::show_toast`]
//! / [`NotificationsState::dismiss_toast`] commands — never by direct field
//! writes into this module's state.

use super::*;

use crate::notification::event::{
    NotificationActionTarget, NotificationEvent, NotificationEventKind,
};
use crate::notification::focus::WindowFocusTracker;
use crate::notification::service::NotificationService;

/// DomainState for the notifications & activity domain.
#[derive(Debug)]
pub(crate) struct NotificationsState {
    /// Desktop/OS notification service (preferences, dedupe, backend).
    pub(crate) notification_service: NotificationService,
    /// Window focus tracker — tracks app visibility for notification
    /// suppression. Wired into `AppMessage::WindowFocusChanged`.
    pub(crate) window_focus_tracker: WindowFocusTracker,
    /// In-app toast message shown as an overlay on some screens. `None`
    /// when no toast is visible.
    pub(crate) toast_message: Option<String>,
    /// Auto-dismiss counter for the in-app toast. Decremented by
    /// [`NotificationsState::tick_toast_auto_dismiss`] from the shell's
    /// ~1 Hz ConnMonitorTick (60 per tick ≈ 2 s for a 120-counter toast).
    pub(crate) toast_counter: u32,
    /// Ring buffer (capacity 50) of landing-page Recent Activity events,
    /// newest first.
    pub(crate) recent_activity: VecDeque<RecentActivityEvent>,
    /// Monotonic counter bumped by `ActivityTick` (once per second). It is
    /// included in the Hash dependencies of the Recent Activity and Tunnels
    /// cards (and the Files dashboard recent-activity card) so `iced::lazy`
    /// rebuilds those time-dependent subtrees while the app is idle. The
    /// Online Peers dependency deliberately excludes it so the peers card
    /// stays memoized across idle seconds.
    pub(crate) activity_tick: u64,
}

impl NotificationsState {
    /// Create the notifications/activity domain state.
    pub(crate) fn new() -> Self {
        Self {
            notification_service: NotificationService::new(),
            window_focus_tracker: WindowFocusTracker::new(),
            toast_message: None,
            toast_counter: 0,
            recent_activity: VecDeque::with_capacity(50),
            activity_tick: 0,
        }
    }

    /// Apply one domain message.
    ///
    /// Only this domain's state is mutated. None of the current messages
    /// require a shell side effect, so no event is returned; the shell just
    /// routes the matching `AppMessage` variant here.
    pub(crate) fn update(&mut self, msg: NotificationsMessage) {
        match msg {
            NotificationsMessage::WindowFocusChanged(focused) => {
                if focused {
                    self.window_focus_tracker.on_focused();
                } else {
                    self.window_focus_tracker.on_unfocused();
                }
            }
            NotificationsMessage::DismissToast => {
                self.toast_message = None;
                self.toast_counter = 0;
            }
            NotificationsMessage::ActivityTick => {
                self.activity_tick = self.activity_tick.wrapping_add(1);
            }
        }
    }

    /// Show the in-app toast overlay for the given message with the given
    /// auto-dismiss counter value (≈ counter/60 seconds at the ~1 Hz
    /// ConnMonitorTick rate — 120 ≈ 2 s, 160 ≈ 2.7 s, 200 ≈ 3.3 s).
    ///
    /// This is the typed command other domains use to surface a toast; they
    /// never write `toast_message`/`toast_counter` directly.
    pub(crate) fn show_toast(&mut self, message: impl Into<String>, counter: u32) {
        self.toast_message = Some(message.into());
        self.toast_counter = counter;
    }

    /// Dismiss the in-app toast immediately.
    pub(crate) fn dismiss_toast(&mut self) {
        self.toast_message = None;
        self.toast_counter = 0;
    }

    /// Show a toast message WITHOUT touching the auto-dismiss counter.
    ///
    /// Used by paths that historically set only `toast_message` (no counter),
    /// preserving the previous behaviour: the toast stays until another toast
    /// replaces it or a counter-driven dismiss clears it.
    pub(crate) fn show_toast_message(&mut self, message: impl Into<String>) {
        self.toast_message = Some(message.into());
    }

    /// Increment the toast auto-dismiss counter.
    ///
    /// Used by repeated-failure paths (e.g. consecutive GIF send failures)
    /// so a stream of failures keeps the toast visible a little longer.
    pub(crate) fn bump_toast_counter(&mut self) {
        self.toast_counter = self.toast_counter.wrapping_add(1);
    }

    /// Auto-dismiss the in-app toast. Called from the shell's ~1 Hz
    /// ConnMonitorTick: decrements the counter by 60 per tick (the counter
    /// was originally intended for 60 fps rendering ticks; the 1 Hz tick
    /// scales it back to the same ~2-second intent).
    pub(crate) fn tick_toast_auto_dismiss(&mut self) {
        if self.toast_counter > 0 {
            self.toast_counter = self.toast_counter.saturating_sub(60);
            if self.toast_counter == 0 {
                self.toast_message = None;
            }
        }
    }

    /// Push a recent activity event for the landing page (ring buffer,
    /// newest first).
    pub(crate) fn push_activity(&mut self, description: impl Into<String>, kind: ActivityKind) {
        if self.recent_activity.len() >= 50 {
            self.recent_activity.pop_back();
        }
        self.recent_activity
            .push_front(RecentActivityEvent::with_kind(description, kind));
    }
}

/// DomainMessage — messages the notifications/activity domain understands.
///
/// The App keeps `AppMessage` as the single app-level message type; the
/// shell routes the matching `AppMessage` variants to
/// [`NotificationsState::update`] (BORU-APP-002 pattern).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotificationsMessage {
    /// The native window gained/lost focus (notification suppression).
    WindowFocusChanged(bool),
    /// Dismiss the in-app toast overlay.
    DismissToast,
    /// Once-per-second tick that refreshes time-dependent lazy cards.
    ActivityTick,
}

/// A recent event shown in the landing-page activity feed.
#[derive(Debug, Clone)]
pub(crate) struct RecentActivityEvent {
    /// Human-readable description.
    pub description: String,
    /// When the event occurred.
    pub timestamp: SystemTime,
    /// Kind of activity — drives the icon selection in the home screen.
    pub kind: ActivityKind,
}

/// Categorises a recent-activity event so the home screen can show a
/// context-appropriate icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ActivityKind {
    Online,
    Offline,
    FileShared,
    Message,
    Generic,
}

impl RecentActivityEvent {
    fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            timestamp: SystemTime::now(),
            kind: ActivityKind::Generic,
        }
    }

    fn with_kind(description: impl Into<String>, kind: ActivityKind) -> Self {
        Self {
            description: description.into(),
            timestamp: SystemTime::now(),
            kind,
        }
    }
}

/// Build the in-app toast overlay on top of `base`.
///
/// Extracted from the friend-profile screen builder (BORU-APP-004): the
/// toast is a notifications-domain view, but it is composed by whichever
/// screen renders it, so this helper takes the base layer and overlays the
/// toast on top. FONTS-15: the toast text renders in the wider IBM Plex
/// Sans default font, so long messages (e.g. "Alice shared a very long
/// tunnel service name with you (2h)") can exceed the window on narrow
/// layouts — the toast width is capped and the text wraps instead of
/// spilling past the window edge.
pub(crate) fn view_toast<'a>(
    base: impl Into<iced::Element<'a, AppMessage>>,
    msg: &'a str,
) -> iced::Element<'a, AppMessage> {
    use iced::widget::{container, text};

    let base: iced::Element<'a, AppMessage> = base.into();
    let toast = container(
        text(msg)
            .size(TYPO_SM)
            .color(iced::Color::WHITE)
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
    )
    .max_width(480.0)
    .padding(iced::Padding {
        top: SPACE_8,
        right: SPACE_16,
        bottom: SPACE_8,
        left: SPACE_16,
    })
    .style(move |t| iced::widget::container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgba(
            0.1, 0.1, 0.1, 0.85,
        ))),
        border: iced::Border {
            radius: SPACE_8.into(),
            ..Default::default()
        },
        ..Default::default()
    });

    iced::widget::stack![
        base,
        container(toast)
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .padding(iced::Padding {
                top: 16.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            }),
    ]
    .into()
}

impl IcedChat {
    /// Build and emit a notification event for a user-visible NetEvent.
    ///
    /// For group conversations the title is the group name and the body is
    /// "Sender: message preview". For direct conversations the title is the
    /// sender's display name and the body is the message preview.
    fn emit_message_notification(
        &mut self,
        topic: &TopicId,
        from: &PublicKey,
        message: &crate::Message,
    ) {
        if !self
            .notifications_state
            .notification_service
            .preferences
            .messages
        {
            return;
        }

        // Determine conversation type and display names
        let is_group = self
            .conversation_store
            .find(topic)
            .map(|entry| entry.kind == ConversationKind::Group)
            .unwrap_or(false);

        let sender_name = self.resolve_name(from);
        let body_text = match message {
            crate::Message::Message { text } => text.clone(),
            // PAPIRUS-10: no emoji as file-type icons in notifications —
            // the OS notification backend renders plain text, so a text
            // label carries the file type instead of an emoji glyph.
            crate::Message::FileShare { name, .. } => format!("File: {name}"),
            crate::Message::ImageShare { .. } => "Image".to_string(),
            crate::Message::SharedGif { .. } => "GIF".to_string(),
            _ => "New message".to_string(),
        };
        // Limit preview length for notification bodies
        let preview = body_text.chars().take(200).collect::<String>();

        let (title, body) = if is_group {
            // Group notification: title = group name, body = "Sender: message"
            let group_name = self
                .conversation_store
                .find(topic)
                .map(|e| e.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "Group".to_string());
            (group_name, format!("{}: {}", sender_name, preview))
        } else {
            // Direct notification: title = sender, body = message preview
            (sender_name, preview)
        };

        let focus = self
            .notifications_state
            .window_focus_tracker
            .to_focus_state();
        let event = NotificationEvent::new(
            NotificationEventKind::NewMessage,
            Some(*from),
            Some(*topic),
            title,
            body,
            Some(NotificationActionTarget::OpenConversation(*topic)),
        );
        self.notifications_state
            .notification_service
            .handle_event(&event, &focus);
    }

    /// Emit an incoming-call notification through the existing notification
    /// service. The service suppresses it while the window is focused, so the
    /// overlay remains the only in-app affordance in that case.
    pub(crate) fn emit_incoming_call_notification(&mut self, peer: &PublicKey) {
        let mut event = NotificationEvent::new(
            NotificationEventKind::IncomingCall,
            Some(*peer),
            None,
            self.resolve_name(peer),
            "Incoming call",
            Some(NotificationActionTarget::OpenChatList),
        );
        event.priority = crate::notification::event::NotificationPriority::High;
        let focus = self
            .notifications_state
            .window_focus_tracker
            .to_focus_state();
        self.notifications_state
            .notification_service
            .handle_event(&event, &focus);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> NotificationsState {
        NotificationsState::new()
    }

    #[test]
    fn window_focus_changed_updates_focus_tracker() {
        let mut s = state();
        assert!(s.window_focus_tracker.window_focused);
        s.update(NotificationsMessage::WindowFocusChanged(false));
        assert!(!s.window_focus_tracker.window_focused);
        s.update(NotificationsMessage::WindowFocusChanged(true));
        assert!(s.window_focus_tracker.window_focused);
    }

    #[test]
    fn dismiss_toast_clears_message_and_counter() {
        let mut s = state();
        s.show_toast("hello", 160);
        assert_eq!(s.toast_message.as_deref(), Some("hello"));
        assert_eq!(s.toast_counter, 160);
        s.update(NotificationsMessage::DismissToast);
        assert!(s.toast_message.is_none());
        assert_eq!(s.toast_counter, 0);
    }

    #[test]
    fn activity_tick_bumps_the_revision() {
        let mut s = state();
        assert_eq!(s.activity_tick, 0);
        s.update(NotificationsMessage::ActivityTick);
        assert_eq!(s.activity_tick, 1);
        s.update(NotificationsMessage::ActivityTick);
        assert_eq!(s.activity_tick, 2);
    }

    #[test]
    fn toast_auto_dismiss_expires_after_counter_elapses() {
        let mut s = state();
        // 120 counter ≈ 2 s at the ~1 Hz ConnMonitorTick (60/tick).
        s.show_toast("temporary", 120);
        s.tick_toast_auto_dismiss();
        assert_eq!(s.toast_counter, 60);
        assert!(s.toast_message.is_some());
        s.tick_toast_auto_dismiss();
        assert_eq!(s.toast_counter, 0);
        assert!(s.toast_message.is_none());
        // Idle ticks keep it dismissed.
        s.tick_toast_auto_dismiss();
        assert!(s.toast_message.is_none());
    }

    #[test]
    fn push_activity_keeps_newest_first_ring_buffer() {
        let mut s = state();
        for i in 0..5 {
            s.push_activity(format!("event {i}"), ActivityKind::Generic);
        }
        assert_eq!(s.recent_activity.len(), 5);
        assert_eq!(s.recent_activity[0].description, "event 4");
        assert_eq!(s.recent_activity[4].description, "event 0");
    }

    #[test]
    fn push_activity_caps_at_50_events() {
        let mut s = state();
        for i in 0..60 {
            s.push_activity(format!("event {i}"), ActivityKind::Message);
        }
        assert_eq!(s.recent_activity.len(), 50);
        // Newest first: event 59 at the front, event 10 at the back.
        assert_eq!(s.recent_activity[0].description, "event 59");
        assert_eq!(s.recent_activity[49].description, "event 10");
    }

    #[test]
    fn show_toast_with_kind_does_not_disturb_activity() {
        let mut s = state();
        s.push_activity("online", ActivityKind::Online);
        s.show_toast("toast", 160);
        assert_eq!(s.recent_activity.len(), 1);
        assert_eq!(s.toast_message.as_deref(), Some("toast"));
    }
}
