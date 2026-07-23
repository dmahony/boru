//! Notification event types for Boru Chat.
//!
//! This module defines the internal notification event model — a platform-neutral
//! representation of events that may produce user-facing notifications.
//!
//! Events are emitted by the application's event handlers and consumed by
//! [`NotificationService`](super::service::NotificationService).
//!
//! # Design rules
//!
//! - Use stable identifiers (PublicKey, TopicId) instead of display text.
//! - Keep peer identity separate from display text.
//! - Do not store rendered notification strings here — title/body are generated
//!   later by the renderer according to privacy settings.
//! - Event types are extensible via the `NotificationEventKind` enum.
//! - No platform-specific fields in this core event type.
//! - Serialisable for diagnostics and potentially for persistence.

use iroh::PublicKey;
use std::time::{SystemTime, UNIX_EPOCH};

use boru_core::proto::TopicId;

// ── Core event types ───────────────────────────────────────────────────────

/// Kinds of notification events that can occur.
///
/// Each variant identifies the category of event so the notification service
/// can apply the correct rendering, deduplication, grouping, and privacy rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationEventKind {
    /// A new text message, file share, or image share arrived.
    NewMessage,
    /// An incoming friend request from another peer.
    FriendRequest,
    /// A friend request we sent was accepted.
    FriendRequestAccepted,
    /// A file transfer completed successfully.
    FileTransferCompleted,
    /// A file transfer failed.
    FileTransferFailed,
    /// The network connection to the mesh was lost.
    ConnectionLost,
    /// The network connection to the mesh was restored.
    ConnectionRestored,
}

impl NotificationEventKind {
    /// Human-readable category label for diagnostics and settings.
    pub fn label(&self) -> &'static str {
        match self {
            Self::NewMessage => "New message",
            Self::FriendRequest => "Friend request",
            Self::FriendRequestAccepted => "Friend request accepted",
            Self::FileTransferCompleted => "File transfer completed",
            Self::FileTransferFailed => "File transfer failed",
            Self::ConnectionLost => "Connection lost",
            Self::ConnectionRestored => "Connection restored",
        }
    }
}

/// Priority level for the notification — controls whether the notification
/// is shown during Do Not Disturb and whether it bypasses rate limiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NotificationPriority {
    /// Low priority — grouped, may be suppressed during DND.
    Low,
    /// Normal priority — shown according to user preferences.
    Normal,
    /// High priority — bypasses rate limiting, shown during DND if configured.
    High,
}

impl Default for NotificationPriority {
    fn default() -> Self {
        Self::Normal
    }
}

/// A stable action target describing what screen to navigate to when a
/// notification is clicked or when the user presses an action button.
///
/// Action targets are parsed from structured data — never from displayed text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NotificationActionTarget {
    /// Open the chat conversation with the given topic.
    OpenConversation(TopicId),
    /// Open the friend request management screen.
    OpenFriendRequests,
    /// Open the file transfers/downloads view (topic context if available).
    OpenTransfers(Option<TopicId>),
    /// Open the main chat list (conversation index).
    OpenChatList,
    /// Open the settings page.
    OpenSettings,
}

impl NotificationActionTarget {
    /// Human-readable label for diagnostics.
    pub fn label(&self) -> &'static str {
        match self {
            Self::OpenConversation(_) => "open-conversation",
            Self::OpenFriendRequests => "open-friend-requests",
            Self::OpenTransfers(_) => "open-transfers",
            Self::OpenChatList => "open-chat-list",
            Self::OpenSettings => "open-settings",
        }
    }
}

/// A notification action that the user can take on a notification.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NotificationAction {
    /// Stable action identifier (e.g. "open", "mark_read", "accept", "decline").
    /// Used to route the action back to the handler.
    pub id: String,
    /// Human-readable label shown on the action button.
    pub label: String,
}

// ── Internal notification event ────────────────────────────────────────────

/// An internal notification event — the core data type that flows through the
/// notification system.
///
/// # Fields
///
/// * `notification_id` — Stable unique identifier for deduplication.
/// * `event_kind` — The type of event that occurred.
/// * `peer_id` — The peer this event is about, if applicable.
/// * `conversation_id` — The conversation topic this event belongs to, if any.
/// * `title_hint` — A stable hint for the title (e.g. sender's display name
///   or a fixed label like "Boru Chat"). This is NOT the rendered title —
///   the renderer applies privacy rules before producing the final string.
/// * `body_hint` — Stable hint for the body (e.g. message preview, filename,
///   request text). Rendered later according to privacy settings.
/// * `timestamp` — When the event occurred (Unix epoch milliseconds).
/// * `priority` — Relative importance of this notification.
/// * `group_key` — Used to group related notifications (e.g. by conversation).
/// * `action_target` — Where to navigate when the notification is clicked.
/// * `actions` — Optional list of user-selectable actions.
#[derive(Debug, Clone)]
pub struct NotificationEvent {
    /// Stable notification ID for deduplication and dismissal.
    /// Format: `{kind}:{conversation_id}:{message_hash_or_sequence}`.
    pub notification_id: String,
    /// The kind of event.
    pub event_kind: NotificationEventKind,
    /// The peer this event is about (sender of message, requester, etc.),
    /// or `None` for system-level events (connection loss).
    pub peer_id: Option<PublicKey>,
    /// The conversation topic this event belongs to, if applicable.
    pub conversation_id: Option<TopicId>,
    /// Stable hint for the notification title (e.g. a display name or event label).
    /// NOT the final rendered title — subject to privacy rendering.
    pub title_hint: String,
    /// Stable hint for the notification body (e.g. a message text snippet).
    /// NOT the final rendered body — subject to privacy rendering.
    pub body_hint: String,
    /// Unix epoch milliseconds when the event occurred.
    pub timestamp: u64,
    /// Priority level for this notification.
    pub priority: NotificationPriority,
    /// Group key for coalescing related notifications (e.g. conversation topic).
    /// `None` means the notification is a singleton (e.g. connection change).
    pub group_key: Option<String>,
    /// Where to navigate when the notification is clicked.
    pub action_target: Option<NotificationActionTarget>,
    /// Optional list of user-selectable notification actions.
    pub actions: Vec<NotificationAction>,
}

impl NotificationEvent {
    /// Create a new notification event with the current timestamp.
    pub fn new(
        event_kind: NotificationEventKind,
        peer_id: Option<PublicKey>,
        conversation_id: Option<TopicId>,
        title_hint: impl Into<String>,
        body_hint: impl Into<String>,
        action_target: Option<NotificationActionTarget>,
    ) -> Self {
        let now = now_unix_ms();
        let notification_id = format!("{:?}:{}:{}",
            event_kind,
            conversation_id.map(|t| t.to_string()).unwrap_or_default(),
            now,
        );
        Self {
            notification_id,
            event_kind,
            peer_id,
            conversation_id,
            title_hint: title_hint.into(),
            body_hint: body_hint.into(),
            timestamp: now,
            priority: NotificationPriority::Normal,
            group_key: conversation_id.map(|t| t.to_string()),
            action_target,
            actions: Vec::new(),
        }
    }

    /// Set a custom notification ID for deduplication (e.g. when the ID
    /// should be based on a protocol message hash rather than a timestamp).
    pub fn with_notification_id(mut self, id: impl Into<String>) -> Self {
        self.notification_id = id.into();
        self
    }

    /// Set the priority level.
    pub fn with_priority(mut self, priority: NotificationPriority) -> Self {
        self.priority = priority;
        self
    }

    /// Set the group key for grouping related notifications.
    pub fn with_group_key(mut self, key: impl Into<String>) -> Self {
        self.group_key = Some(key.into());
        self
    }

    /// Add a user-selectable action to this notification.
    pub fn with_action(mut self, action: NotificationAction) -> Self {
        self.actions.push(action);
        self
    }

    /// Set the timestamp explicitly (for replaying historical events).
    pub fn with_timestamp(mut self, ts: u64) -> Self {
        self.timestamp = ts;
        self
    }
}

// ── Helper ─────────────────────────────────────────────────────────────────

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_peer() -> PublicKey {
        use iroh::SecretKey;
        SecretKey::generate().public()
    }

    #[test]
    fn test_new_message_event_creation() {
        let peer = test_peer();
        let event = NotificationEvent::new(
            NotificationEventKind::NewMessage,
            Some(peer),
            None,
            "Alice",
            "Hello, how are you?",
            None,
        );
        assert_eq!(event.event_kind, NotificationEventKind::NewMessage);
        assert_eq!(event.peer_id, Some(peer));
        assert_eq!(event.title_hint, "Alice");
        assert_eq!(event.body_hint, "Hello, how are you?");
        assert!(event.timestamp > 0);
        assert!(event.notification_id.starts_with("NewMessage:"));
    }

    #[test]
    fn test_friend_request_event() {
        let peer = test_peer();
        let event = NotificationEvent::new(
            NotificationEventKind::FriendRequest,
            Some(peer),
            None,
            "Alice",
            "wants to connect",
            Some(NotificationActionTarget::OpenFriendRequests),
        )
        .with_action(NotificationAction {
            id: "accept".into(),
            label: "Accept".into(),
        })
        .with_action(NotificationAction {
            id: "decline".into(),
            label: "Decline".into(),
        });

        assert_eq!(event.action_target, Some(NotificationActionTarget::OpenFriendRequests));
        assert_eq!(event.actions.len(), 2);
        assert_eq!(event.actions[0].id, "accept");
        assert_eq!(event.actions[1].id, "decline");
    }

    #[test]
    fn test_priority_default() {
        let peer = test_peer();
        let event = NotificationEvent::new(
            NotificationEventKind::NewMessage,
            Some(peer),
            None,
            "Alice",
            "Hello",
            None,
        );
        assert_eq!(event.priority, NotificationPriority::Normal);
    }

    #[test]
    fn test_high_priority() {
        let event = NotificationEvent::new(
            NotificationEventKind::ConnectionLost,
            None,
            None,
            "Boru Chat",
            "is offline",
            None,
        )
        .with_priority(NotificationPriority::High);
        assert_eq!(event.priority, NotificationPriority::High);
    }

    #[test]
    fn test_group_key_from_conversation() {
        let peer = test_peer();
        let topic = TopicId::from([0u8; 32]);
        let event = NotificationEvent::new(
            NotificationEventKind::NewMessage,
            Some(peer),
            Some(topic),
            "Alice",
            "Hello",
            None,
        );
        assert_eq!(event.group_key, Some(topic.to_string()));
    }

    #[test]
    fn test_event_kind_label() {
        assert_eq!(NotificationEventKind::NewMessage.label(), "New message");
        assert_eq!(NotificationEventKind::FriendRequest.label(), "Friend request");
        assert_eq!(NotificationEventKind::ConnectionLost.label(), "Connection lost");
        assert_eq!(NotificationEventKind::ConnectionRestored.label(), "Connection restored");
    }

    #[test]
    fn test_action_target_label() {
        let topic = TopicId::from([0u8; 32]);
        assert_eq!(NotificationActionTarget::OpenConversation(topic).label(), "open-conversation");
        assert_eq!(NotificationActionTarget::OpenFriendRequests.label(), "open-friend-requests");
        assert_eq!(NotificationActionTarget::OpenChatList.label(), "open-chat-list");
        assert_eq!(NotificationActionTarget::OpenSettings.label(), "open-settings");
    }

    #[test]
    fn test_custom_notification_id() {
        let event = NotificationEvent::new(
            NotificationEventKind::NewMessage,
            None,
            None,
            "Alice",
            "Hello",
            None,
        )
        .with_notification_id("custom_id_123");
        assert_eq!(event.notification_id, "custom_id_123");
    }

    #[test]
    fn test_conversation_id_defaults_to_group_key() {
        let topic = TopicId::from([1u8; 32]);
        let event = NotificationEvent::new(
            NotificationEventKind::NewMessage,
            None,
            Some(topic),
            "Bob",
            "Hey",
            None,
        );
        assert_eq!(event.group_key, Some(topic.to_string()));
    }

    #[test]
    fn test_file_transfer_completed() {
        let event = NotificationEvent::new(
            NotificationEventKind::FileTransferCompleted,
            None,
            None,
            "Boru Chat",
            "File transfer completed",
            None,
        );
        assert_eq!(event.event_kind, NotificationEventKind::FileTransferCompleted);
        assert_eq!(event.title_hint, "Boru Chat");
    }

    #[test]
    fn test_file_transfer_failed() {
        let event = NotificationEvent::new(
            NotificationEventKind::FileTransferFailed,
            None,
            None,
            "Boru Chat",
            "Could not receive archive.zip",
            None,
        );
        assert_eq!(event.event_kind, NotificationEventKind::FileTransferFailed);
    }

    #[test]
    fn test_connection_events() {
        let lost = NotificationEvent::new(
            NotificationEventKind::ConnectionLost,
            None,
            None,
            "Boru Chat",
            "is offline",
            None,
        );
        assert_eq!(lost.event_kind, NotificationEventKind::ConnectionLost);

        let restored = NotificationEvent::new(
            NotificationEventKind::ConnectionRestored,
            None,
            None,
            "Boru Chat",
            "is online again",
            None,
        );
        assert_eq!(restored.event_kind, NotificationEventKind::ConnectionRestored);
    }
}
