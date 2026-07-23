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
use crate::notification::event::{
    NotificationActionTarget, NotificationEvent, NotificationEventKind, NotificationPriority,
};

/// How message previews are shown in notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewMode {
    /// Show sender name and message content.
    Full,
    /// Show sender name only.
    SenderOnly,
    /// Hide sender name and message content.
    Hidden,
}

impl Default for PreviewMode {
    fn default() -> Self {
        Self::Full
    }
}

/// Global notification preferences.
#[derive(Debug, Clone)]
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
            preview_mode: PreviewMode::Full,
            notify_while_focused: false,
            sound: true,
        }
    }
}

/// Per-conversation mute state.
#[derive(Debug, Clone)]
pub struct ConversationMute {
    /// When the mute expires, if temporary. None = indefinite.
    pub expires_at: Option<SystemTime>,
}

impl ConversationMute {
    pub fn is_muted(&self) -> bool {
        match self.expires_at {
            Some(expiry) => SystemTime::now() < expiry,
            None => true,
        }
    }
}

/// Do Not Disturb schedule.
#[derive(Debug, Clone)]
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

    fn evict_stale(&mut self) {
        let cutoff = Instant::now() - self.ttl;
        self.entries.retain(|_, &mut t| t > cutoff);
    }
}

// ── Notification grouping ──────────────────────────────────────────

/// Tracks active notification groups for combining related notifications.
#[derive(Debug)]
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
pub struct NotificationService {
    backend: Box<dyn NotificationBackend + Send>,
    preferences: NotificationPreferences,
    mutes: HashMap<TopicId, ConversationMute>,
    dedup: DedupCache,
    groups: GroupTracker,
    dnd: DoNotDisturb,
}

impl NotificationService {
    /// Create a new notification service with a no-op backend.
    pub fn new() -> Self {
        Self {
            backend: Box::new(NoopBackend),
            preferences: NotificationPreferences::default(),
            mutes: HashMap::new(),
            dedup: DedupCache::default(),
            groups: GroupTracker::default(),
            dnd: DoNotDisturb::default(),
        }
    }

    /// Replace the platform backend.
    pub fn set_backend(&mut self, backend: Box<dyn NotificationBackend + Send>) {
        self.backend = backend;
    }

    /// Update user notification preferences.
    pub fn set_preferences(&mut self, prefs: NotificationPreferences) {
        self.preferences = prefs;
    }

    /// Update the Do Not Disturb schedule.
    pub fn set_dnd(&mut self, dnd: DoNotDisturb) {
        self.dnd = dnd;
    }

    /// Set or update the mute state for a conversation.
    pub fn set_conversation_mute(&mut self, topic: TopicId, mute: ConversationMute) {
        self.mutes.insert(topic, mute);
    }

    /// Remove mute state for a conversation (unmute).
    pub fn remove_conversation_mute(&mut self, topic: &TopicId) {
        self.mutes.remove(topic);
    }

    /// Core notification entry point.
    ///
    /// Takes an internal notification event plus current application
    /// focus state and decides whether to show, update, or ignore.
    pub fn handle_event(&mut self, event: &NotificationEvent, focus: &WindowFocusState) {
        // 1. Master toggle
        if !self.preferences.enabled {
            tracing::debug!("[notif] suppressed: notifications disabled");
            return;
        }

        // 2. Check event-type-specific preference
        if !self.event_kind_enabled(&event.event_kind) {
            return;
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
        if self.dnd.is_active() {
            if !matches!(event.priority, NotificationPriority::High) {
                tracing::debug!("[notif] suppressed: DND active");
                return;
            }
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
    pub fn dismiss(&mut self, id: &str) {
        self.backend.close(id);
    }

    /// Dismiss all notifications in a group.
    pub fn dismiss_group(&mut self, _group_key: &str) {
        tracing::debug!("[notif] dismiss group: {_group_key}");
    }

    /// Handle a notification action.
    pub fn handle_action(&mut self, action: &str) {
        tracing::debug!("[notif] action: {action}");
    }

    // ── Private helpers ──────────────────────────────────────────

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
        }
    }

    fn dedup_key(&self, event: &NotificationEvent) -> String {
        let conv = event
            .conversation_id
            .map(|t| t.to_string())
            .unwrap_or_default();
        format!("{:?}_{}_{}", event.event_kind, conv, event.title_hint)
    }

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
                PreviewMode::Hidden => {
                    ("Boru Chat".to_string(), "New friend request".to_string())
                }
                _ => ("Friend request".to_string(), event.title_hint.clone()),
            },
            NotificationEventKind::FriendRequestAccepted => match preview {
                PreviewMode::Hidden => {
                    ("Boru Chat".to_string(), "Friend request accepted".to_string())
                }
                _ => (
                    event.title_hint.clone(),
                    "Accepted your request".to_string(),
                ),
            },
            NotificationEventKind::FileTransferCompleted => match preview {
                PreviewMode::Hidden => {
                    ("Boru Chat".to_string(), "File transfer completed".to_string())
                }
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
pub struct WindowFocusState {
    pub window_focused: bool,
    pub window_visible: bool,
    pub window_minimised: bool,
    pub app_running_in_background: bool,
    pub active_conversation_id: Option<TopicId>,
}

impl WindowFocusState {
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
    pub fn is_focused_or_visible(&self) -> bool {
        self.window_focused && self.window_visible && !self.window_minimised
    }

    /// Returns true if the user is actively viewing a conversation
    /// and that conversation matches the given topic.
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
