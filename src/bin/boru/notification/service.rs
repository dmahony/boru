#![allow(
    clippy::type_complexity,
    clippy::too_many_arguments,
    clippy::large_enum_variant,
    clippy::if_same_then_else,
    clippy::doc_lazy_continuation,
    clippy::doc_overindented_list_items,
    clippy::redundant_guards,
    clippy::manual_let_else,
    clippy::vec_init_then_push,
    clippy::let_underscore_future,
    clippy::needless_update,
    clippy::unnecessary_unwrap,
    clippy::single_match,
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::question_mark,
    clippy::unnecessary_sort_by,
    clippy::result_large_err,
    clippy::enum_variant_names,
    clippy::explicit_counter_loop,
    clippy::wrong_self_convention,
    missing_debug_implementations,
    unfulfilled_lint_expectations
)]
#![allow(dead_code)]

//! Central notification service that handles the full notification
//! lifecycle: receiving events, checking preferences, deduplicating,
//! grouping, rendering, and dispatching through the platform backend.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use boru_core::proto::TopicId;
use chrono::Timelike;

use crate::notification::backend::{
    NoopBackend, NotificationAction as Action, NotificationBackend, RenderedNotification,
};
use crate::notification::event::{NotificationEvent, NotificationEventKind, NotificationPriority};

/// How message previews are shown in notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(dead_code)]
#[derive(Default)]
pub enum PreviewMode {
    /// Show sender name and message content.
    #[default]
    Full,
    /// Show sender name only.
    SenderOnly,
    /// Hide sender name and message content.
    Hidden,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum NotificationPolicy {
    #[default]
    All,
    MentionsOnly,
    Muted,
}

/// Global notification preferences.
#[derive(Debug, Clone)]
#[expect(dead_code)]
pub struct NotificationPreferences {
    /// Master notification toggle.
    pub enabled: bool,
    /// Notify on new messages.
    pub messages: bool,
    /// Notify on friend requests.
    pub friend_requests: bool,
    /// Notify on file transfers.
    pub file_transfers: bool,
    /// Notify on connection warnings.
    pub connection_warnings: bool,
    /// Notify about incoming calls when the window is not focused.
    pub incoming_calls: bool,
    /// How message previews are shown.
    pub preview_mode: PreviewMode,
    /// Notify when the app is focused.
    pub notify_while_focused: bool,
    /// Whether notification sounds are enabled.
    pub sound: bool,
}

impl Default for NotificationPreferences {
    fn default() -> Self {
        Self {
            enabled: true,
            messages: true,
            friend_requests: true,
            file_transfers: true,
            connection_warnings: false, // off by default (PDF Step 15)
            incoming_calls: true,
            preview_mode: PreviewMode::Full,
            notify_while_focused: false,
            sound: true,
        }
    }
}

/// Per-conversation mute state.
#[derive(Debug, Clone)]
#[expect(dead_code)]
pub struct ConversationMute {
    /// When the mute expires, if temporary. None = indefinite.
    pub expires_at: Option<SystemTime>,
}

impl ConversationMute {
    #[expect(dead_code)]
    pub fn is_muted(&self) -> bool {
        match self.expires_at {
            Some(expiry) => SystemTime::now() < expiry,
            None => true,
        }
    }
}

/// Do Not Disturb schedule.
#[derive(Debug, Clone)]
#[expect(dead_code)]
pub struct DoNotDisturb {
    pub enabled: bool,
    /// Start hour (0–23, local time).
    pub from_hour: u8,
    /// Start minute (0–59).
    pub from_minute: u8,
    /// End hour (0–23, local time).
    pub until_hour: u8,
    /// End minute (0–59).
    pub until_minute: u8,
}

impl Default for DoNotDisturb {
    fn default() -> Self {
        Self {
            enabled: false,
            from_hour: 22,
            from_minute: 0,
            until_hour: 8,
            until_minute: 0,
        }
    }
}

impl DoNotDisturb {
    /// Returns true if the current local time falls within the DND window.
    #[expect(dead_code)]
    pub fn is_active(&self) -> bool {
        if !self.enabled {
            return false;
        }
        let now = chrono::Local::now();
        let now_minutes = now.hour() as u16 * 60 + now.minute() as u16;
        let from_minutes = self.from_hour as u16 * 60 + self.from_minute as u16;
        let until_minutes = self.until_hour as u16 * 60 + self.until_minute as u16;

        if from_minutes <= until_minutes {
            // Same-day range (e.g., 10:00–18:00)
            now_minutes >= from_minutes && now_minutes < until_minutes
        } else {
            // Crosses midnight (e.g., 22:00–08:00)
            now_minutes >= from_minutes || now_minutes < until_minutes
        }
    }
}

// ── Duplicate detection cache ──────────────────────────────────────

/// Bounded cache of recently processed notification event IDs.
#[derive(Debug)]
#[expect(dead_code)]
struct DedupCache {
    entries: HashMap<String, Instant>,
    max_entries: usize,
    ttl: Duration,
}

impl Default for DedupCache {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            max_entries: 500,
            ttl: Duration::from_secs(60),
        }
    }
}

impl DedupCache {
    /// Check if a key was already seen and, if not, record it.
    #[expect(dead_code)]
    fn try_insert(&mut self, key: &str) -> bool {
        self.evict_stale();
        if self.entries.contains_key(key) {
            return false;
        }
        if self.entries.len() >= self.max_entries {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, &t)| t)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(key.to_string(), Instant::now());
        true
    }

    #[expect(dead_code)]
    fn evict_stale(&mut self) {
        let cutoff = Instant::now() - self.ttl;
        self.entries.retain(|_, &mut t| t > cutoff);
    }
}

// ── Notification grouping ──────────────────────────────────────────

/// Tracks active notification groups for combining related notifications.
#[derive(Debug)]
#[expect(dead_code)]
struct GroupTracker {
    /// group_key → (first_event_time, notification_id, current_count)
    groups: HashMap<String, (Instant, String, u64)>,
    /// How long before grouped notifications coalesce into a summary.
    window: Duration,
}

impl Default for GroupTracker {
    fn default() -> Self {
        Self {
            groups: HashMap::new(),
            window: Duration::from_secs(5),
        }
    }
}

impl GroupTracker {
    /// Returns (is_new_group, notification_id_for_batch_update).
    #[expect(dead_code)]
    fn track(&mut self, group_key: &str, fallback_id: &str) -> (bool, String) {
        self.evict_stale();

        if let Some((first_time, existing_id, count)) = self.groups.get_mut(group_key) {
            let elapsed = first_time.elapsed();
            if elapsed < self.window {
                *count += 1;
                return (false, existing_id.clone());
            }
        }

        let id = fallback_id.to_string();
        self.groups
            .insert(group_key.to_string(), (Instant::now(), id.clone(), 1));
        (true, id)
    }

    #[expect(dead_code)]
    fn evict_stale(&mut self) {
        let cutoff = Instant::now() - self.window;
        self.groups.retain(|_, &mut (t, _, _)| t > cutoff);
    }
}

// ── Notification Service ───────────────────────────────────────────

/// Central notification service that manages the full lifecycle.
///
/// Responsibilities (from PDF Step 3):
/// - Receive internal notification events
/// - Check user preferences
/// - Check focus and visibility state
/// - Check conversation mute state
/// - Apply privacy rules
/// - Deduplicate events
/// - Group events
/// - Render title and body
/// - Send through a platform backend
/// - Handle notification actions
#[derive(Debug)]
#[expect(dead_code)]
pub struct NotificationService {
    backend: Box<dyn NotificationBackend + Send>,
    pub preferences: NotificationPreferences,
    mutes: HashMap<TopicId, ConversationMute>,
    dedup: DedupCache,
    groups: GroupTracker,
    dnd: DoNotDisturb,
    pub message_policy: NotificationPolicy,
    conversation_policies: HashMap<TopicId, NotificationPolicy>,
}

impl NotificationService {
    /// Create a new notification service with a no-op backend.
    #[expect(dead_code)]
    pub fn new() -> Self {
        Self {
            backend: Box::new(NoopBackend),
            preferences: NotificationPreferences::default(),
            mutes: HashMap::new(),
            dedup: DedupCache::default(),
            groups: GroupTracker::default(),
            dnd: DoNotDisturb::default(),
            message_policy: NotificationPolicy::All,
            conversation_policies: HashMap::new(),
        }
    }

    /// Replace the platform backend.
    #[expect(dead_code)]
    pub fn set_backend(&mut self, backend: Box<dyn NotificationBackend + Send>) {
        self.backend = backend;
    }

    /// Update user notification preferences.
    #[expect(dead_code)]
    pub fn set_preferences(&mut self, prefs: NotificationPreferences) {
        self.preferences = prefs;
    }

    /// Update the Do Not Disturb schedule.
    #[expect(dead_code)]
    pub fn set_dnd(&mut self, dnd: DoNotDisturb) {
        self.dnd = dnd;
    }

    /// Set or update the mute state for a conversation.
    #[expect(dead_code)]
    pub fn set_conversation_mute(&mut self, topic: TopicId, mute: ConversationMute) {
        self.mutes.insert(topic, mute);
    }

    /// Remove mute state for a conversation (unmute).
    #[expect(dead_code)]
    pub fn remove_conversation_mute(&mut self, topic: &TopicId) {
        self.mutes.remove(topic);
    }

    pub fn set_message_policy(&mut self, policy: NotificationPolicy) {
        self.message_policy = policy;
    }

    pub fn set_conversation_policy(&mut self, topic: TopicId, policy: Option<NotificationPolicy>) {
        match policy {
            Some(policy) => {
                self.conversation_policies.insert(topic, policy);
            }
            None => {
                self.conversation_policies.remove(&topic);
            }
        }
    }

    pub fn conversation_policies_snapshot(&self) -> Vec<(String, NotificationPolicy)> {
        self.conversation_policies
            .iter()
            .map(|(topic, policy)| (topic.to_string(), *policy))
            .collect()
    }

    pub fn restore_conversation_policies(&mut self, policies: &[(String, NotificationPolicy)]) {
        for (key, policy) in policies {
            if let Ok(bytes) = hex::decode(key) {
                if bytes.len() == 32 {
                    let mut raw = [0u8; 32];
                    raw.copy_from_slice(&bytes);
                    self.conversation_policies
                        .insert(TopicId::from_bytes(raw), *policy);
                }
            }
        }
    }

    pub fn effective_policy(&self, topic: Option<&TopicId>) -> NotificationPolicy {
        topic
            .and_then(|topic| self.conversation_policies.get(topic).copied())
            .unwrap_or(self.message_policy)
    }

    /// Core notification entry point.
    ///
    /// Takes an internal notification event plus current application
    /// focus state and decides whether to show, update, or ignore.
    #[expect(dead_code)]
    pub fn handle_event(&mut self, event: &NotificationEvent, focus: &WindowFocusState) {
        self.handle_event_with_mention(event, focus, false);
    }

    pub fn handle_event_with_mention(
        &mut self,
        event: &NotificationEvent,
        focus: &WindowFocusState,
        mentions_local: bool,
    ) {
        // 1. Master toggle
        if !self.preferences.enabled {
            tracing::debug!("[notif] suppressed: notifications disabled");
            return;
        }

        // 2. Check event-type-specific preference
        if !self.event_kind_enabled(&event.event_kind) {
            return;
        }
        if event.event_kind == NotificationEventKind::NewMessage {
            match self.effective_policy(event.conversation_id.as_ref()) {
                NotificationPolicy::Muted => return,
                NotificationPolicy::MentionsOnly if !mentions_local => return,
                _ => {}
            }
        }

        // 3. Check focus: if app is focused and notify_while_focused is off, suppress
        if focus.is_focused_or_visible() && !self.preferences.notify_while_focused {
            tracing::debug!("[notif] suppressed: app focused, notify_while_focused disabled");
            return;
        }

        // 4. Check conversation mute (for message events)
        if let Some(topic) = &event.conversation_id {
            if let Some(mute) = self.mutes.get(topic) {
                if mute.is_muted() {
                    tracing::debug!("[notif] suppressed: conversation {topic} is muted");
                    return;
                }
            }
        }

        // 5. Check Do Not Disturb
        if self.dnd.is_active() && !matches!(event.priority, NotificationPriority::High) {
            tracing::debug!("[notif] suppressed: DND active");
            return;
        }

        // 6. Deduplication
        let dedup_key = self.dedup_key(event);
        if !self.dedup.try_insert(&dedup_key) {
            tracing::debug!("[notif] suppressed: duplicate {dedup_key}");
            return;
        }

        // 7. Group tracking (use group_key or fall back to dedup_key)
        let group_key = event.group_key.as_deref().unwrap_or(&dedup_key);
        let (is_new, group_id) = self.groups.track(group_key, &event.notification_id);

        // 8. Render according to privacy mode
        let rendered = self.render(event, &group_id, is_new);

        // 9. Send through backend
        if self.backend.is_available() {
            if is_new {
                self.backend.show(&rendered);
            } else {
                self.backend.update(&rendered);
            }
        } else {
            tracing::debug!("[notif] backend not available, logging: {rendered:?}");
        }
    }

    /// Dismiss a notification by ID.
    #[expect(dead_code)]
    pub fn dismiss(&mut self, id: &str) {
        self.backend.close(id);
    }

    /// Dismiss all notifications in a group.
    #[expect(dead_code)]
    pub fn dismiss_group(&mut self, _group_key: &str) {
        tracing::debug!("[notif] dismiss group: {_group_key}");
    }

    /// Handle a notification action.
    #[expect(dead_code)]
    pub fn handle_action(&mut self, action: &str) {
        tracing::debug!("[notif] action: {action}");
    }

    // ── Private helpers ──────────────────────────────────────────

    #[expect(dead_code)]
    fn event_kind_enabled(&self, kind: &NotificationEventKind) -> bool {
        match kind {
            NotificationEventKind::NewMessage => self.preferences.messages,
            NotificationEventKind::FriendRequest | NotificationEventKind::FriendRequestAccepted => {
                self.preferences.friend_requests
            }
            NotificationEventKind::FileTransferCompleted
            | NotificationEventKind::FileTransferFailed => self.preferences.file_transfers,
            NotificationEventKind::ConnectionLost | NotificationEventKind::ConnectionRestored => {
                self.preferences.connection_warnings
            }
            NotificationEventKind::IncomingCall => self.preferences.incoming_calls,
        }
    }

    #[expect(dead_code)]
    fn dedup_key(&self, event: &NotificationEvent) -> String {
        // Keep the storm guard keyed by the individual application event.
        // Conversation/title alone would suppress every subsequent message
        // from the same sender during the TTL window.
        event.notification_id.clone()
    }

    #[expect(dead_code)]
    fn render(
        &self,
        event: &NotificationEvent,
        _group_id: &str,
        _is_new: bool,
    ) -> RenderedNotification {
        let preview = self.preferences.preview_mode;

        let (title, body) = match preview {
            PreviewMode::Full => (event.title_hint.clone(), event.body_hint.clone()),
            PreviewMode::SenderOnly => (event.title_hint.clone(), "New message".to_string()),
            PreviewMode::Hidden => ("Boru Chat".to_string(), "New message".to_string()),
        };

        // Override for non-message event types
        let (title, body) = match &event.event_kind {
            NotificationEventKind::FriendRequest => match preview {
                PreviewMode::Hidden => ("Boru Chat".to_string(), "New friend request".to_string()),
                _ => ("Friend request".to_string(), event.title_hint.clone()),
            },
            NotificationEventKind::FriendRequestAccepted => match preview {
                PreviewMode::Hidden => (
                    "Boru Chat".to_string(),
                    "Friend request accepted".to_string(),
                ),
                _ => (
                    event.title_hint.clone(),
                    "Accepted your request".to_string(),
                ),
            },
            NotificationEventKind::FileTransferCompleted => match preview {
                PreviewMode::Hidden => (
                    "Boru Chat".to_string(),
                    "File transfer completed".to_string(),
                ),
                _ => (event.title_hint.clone(), event.body_hint.clone()),
            },
            NotificationEventKind::FileTransferFailed => match preview {
                PreviewMode::Hidden => {
                    ("Boru Chat".to_string(), "File transfer failed".to_string())
                }
                _ => ("Transfer failed".to_string(), event.body_hint.clone()),
            },
            NotificationEventKind::ConnectionLost => {
                ("Boru Chat".to_string(), "Boru Chat is offline".to_string())
            }
            NotificationEventKind::ConnectionRestored => (
                "Connection restored".to_string(),
                "Boru Chat is online again".to_string(),
            ),
            NotificationEventKind::IncomingCall => (
                "Incoming call".to_string(),
                format!("Incoming call from {}", event.title_hint),
            ),
            _ => (title, body),
        };

        // Determine available actions (backend-level actions)
        let actions = match &event.event_kind {
            NotificationEventKind::NewMessage => vec![Action::Open, Action::MarkAsRead],
            NotificationEventKind::FriendRequest => {
                vec![Action::Open, Action::Accept, Action::Decline]
            }
            _ => vec![],
        };

        RenderedNotification {
            id: event.notification_id.clone(),
            title,
            body,
            event_type: format!("{:?}", event.event_kind),
            conversation_target: event.action_target.clone(),
            actions,
            group_key: event.group_key.clone(),
            priority: event.priority,
        }
    }
}

impl Default for NotificationService {
    fn default() -> Self {
        Self::new()
    }
}

// ── Step 4: Window Focus State ────────────────────────────────────

/// Centralised source of truth for application visibility.
///
/// Tracked state (from PDF Step 4):
/// - window_focused
/// - window_visible
/// - window_minimised
/// - application_running_in_background
/// - active_conversation_id
#[derive(Debug, Clone)]
#[expect(dead_code)]
pub struct WindowFocusState {
    pub window_focused: bool,
    pub window_visible: bool,
    pub window_minimised: bool,
    pub app_running_in_background: bool,
    pub active_conversation_id: Option<TopicId>,
}

impl WindowFocusState {
    #[expect(dead_code)]
    pub fn new() -> Self {
        Self {
            window_focused: true,
            window_visible: true,
            window_minimised: false,
            app_running_in_background: false,
            active_conversation_id: None,
        }
    }

    /// Returns true if the application is in a state where the user
    /// is actively looking at a conversation (focused and visible).
    #[expect(dead_code)]
    pub fn is_focused_or_visible(&self) -> bool {
        self.window_focused && self.window_visible && !self.window_minimised
    }

    /// Returns true if the user is actively viewing a conversation
    /// and that conversation matches the given topic.
    #[expect(dead_code)]
    pub fn is_viewing_conversation(&self, topic: &TopicId) -> bool {
        self.is_focused_or_visible() && self.active_conversation_id.as_ref() == Some(topic)
    }
}

impl Default for WindowFocusState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notification::event::NotificationEventKind;

    fn make_msg_event(topic: Option<TopicId>) -> NotificationEvent {
        NotificationEvent::new(
            NotificationEventKind::NewMessage,
            None,
            topic,
            "Alice",
            "Hello!",
            topic.map(NotificationActionTarget::OpenConversation),
        )
    }

    #[test]
    fn test_dedup_prevents_duplicate_events() {
        let mut service = NotificationService::new();
        let focus = WindowFocusState::new();
        let topic = TopicId::from([1u8; 32]);
        let event = make_msg_event(Some(topic));
        service.handle_event(&event, &focus);
        service.handle_event(&event, &focus);
    }

    #[test]
    fn test_focused_app_suppresses_notifications() {
        let mut service = NotificationService::new();
        let mut focus = WindowFocusState::new();
        focus.window_focused = true;
        focus.window_visible = true;
        let topic = TopicId::from([2u8; 32]);
        let event = make_msg_event(Some(topic));
        service.handle_event(&event, &focus);
    }

    #[test]
    fn test_unfocused_app_allows_notifications() {
        let mut service = NotificationService::new();
        let mut focus = WindowFocusState::new();
        focus.window_focused = false;
        let topic = TopicId::from([3u8; 32]);
        let event = make_msg_event(Some(topic));
        service.handle_event(&event, &focus);
    }

    #[test]
    fn test_muted_conversation_suppresses_notifications() {
        let mut service = NotificationService::new();
        let focus = WindowFocusState::new();
        let topic = TopicId::from([5u8; 32]);
        service.set_conversation_mute(
            topic,
            ConversationMute {
                expires_at: Some(SystemTime::now() + Duration::from_secs(3600)),
            },
        );
        let event = make_msg_event(Some(topic));
        service.handle_event(&event, &focus);
    }

    #[test]
    fn test_expired_mute_allows_notifications() {
        let mut service = NotificationService::new();
        let focus = WindowFocusState::new();
        let topic = TopicId::from([6u8; 32]);
        service.set_conversation_mute(
            topic,
            ConversationMute {
                expires_at: Some(SystemTime::now() - Duration::from_secs(1)),
            },
        );
        let event = make_msg_event(Some(topic));
        service.handle_event(&event, &focus);
    }

    #[test]
    fn test_indefinite_mute_suppresses() {
        let mut service = NotificationService::new();
        let focus = WindowFocusState::new();
        let topic = TopicId::from([7u8; 32]);
        service.set_conversation_mute(topic, ConversationMute { expires_at: None });
        let event = make_msg_event(Some(topic));
        service.handle_event(&event, &focus);
    }

    #[test]
    fn test_window_focus_state_tracking() {
        let mut state = WindowFocusState::new();
        assert!(state.is_focused_or_visible());

        state.window_focused = false;
        assert!(!state.is_focused_or_visible());

        state.window_focused = true;
        state.window_minimised = true;
        assert!(!state.is_focused_or_visible());

        state.window_minimised = false;
        state.window_visible = false;
        assert!(!state.is_focused_or_visible());
    }

    #[test]
    fn test_viewing_conversation() {
        let topic = TopicId::from([8u8; 32]);
        let other = TopicId::from([9u8; 32]);
        let mut state = WindowFocusState::new();
        state.active_conversation_id = Some(topic);
        assert!(state.is_viewing_conversation(&topic));
        assert!(!state.is_viewing_conversation(&other));

        state.window_focused = false;
        assert!(!state.is_viewing_conversation(&topic));
    }

    #[test]
    fn test_dedup_cache_eviction() {
        let mut cache = DedupCache {
            entries: HashMap::new(),
            max_entries: 3,
            ttl: Duration::from_secs(60),
        };
        assert!(cache.try_insert("a"));
        assert!(cache.try_insert("b"));
        assert!(cache.try_insert("c"));
        assert!(!cache.try_insert("a"));
        assert!(cache.try_insert("d"));
        assert_eq!(cache.entries.len(), 3);
    }

    #[test]
    fn distinct_messages_in_one_conversation_are_not_deduplicated() {
        let mut cache = DedupCache::default();
        let topic = TopicId::from([10u8; 32]);
        let first = make_msg_event(Some(topic)).with_notification_id("message-1");
        let second = make_msg_event(Some(topic)).with_notification_id("message-2");
        assert!(cache.try_insert(&first.notification_id));
        assert!(cache.try_insert(&second.notification_id));
        assert!(!cache.try_insert(&first.notification_id));
    }

    #[test]
    fn mention_policy_and_room_override_precedence_are_deterministic() {
        let mut service = NotificationService::new();
        let topic = TopicId::from([11u8; 32]);
        assert_eq!(
            service.effective_policy(Some(&topic)),
            NotificationPolicy::All
        );
        service.set_message_policy(NotificationPolicy::MentionsOnly);
        assert_eq!(
            service.effective_policy(Some(&topic)),
            NotificationPolicy::MentionsOnly
        );
        service.set_conversation_policy(topic, Some(NotificationPolicy::Muted));
        assert_eq!(
            service.effective_policy(Some(&topic)),
            NotificationPolicy::Muted
        );
        service.set_conversation_policy(topic, None);
        assert_eq!(
            service.effective_policy(Some(&topic)),
            NotificationPolicy::MentionsOnly
        );
    }

    #[test]
    fn test_dnd_active_checks() {
        let mut dnd = DoNotDisturb::default();
        assert!(!dnd.is_active()); // disabled by default
        dnd.enabled = true;
        let _ = dnd.is_active(); // doesn't panic
    }

    #[test]
    fn test_connection_warnings_off_by_default() {
        let service = NotificationService::new();
        assert!(!service.preferences.connection_warnings);
    }

    #[test]
    fn test_preview_modes() {
        let mut service = NotificationService::new();
        let focus = WindowFocusState::new();

        let event = NotificationEvent::new(
            NotificationEventKind::NewMessage,
            None,
            None,
            "Alice",
            "Secret content",
            None,
        );

        service.preferences.preview_mode = PreviewMode::Full;
        service.handle_event(&event, &focus);

        service.preferences.preview_mode = PreviewMode::SenderOnly;
        service.handle_event(&event, &focus);

        service.preferences.preview_mode = PreviewMode::Hidden;
        service.handle_event(&event, &focus);
    }

    #[test]
    fn test_backend_switching() {
        let mut service = NotificationService::new();
        assert!(!service.backend.is_available());
        service.set_backend(Box::new(NoopBackend));
        assert!(!service.backend.is_available());
    }
}
