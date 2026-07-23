//! Platform-neutral backend interface for notifications.
//!
//! Each desktop platform implements this trait to show, update, and close
//! notifications using the native notification API. A no-op backend is
//! provided for unsupported platforms and tests.

use crate::notification::event::{NotificationActionTarget, NotificationEvent, NotificationPriority};

/// A rendered notification ready for display by a backend.
///
/// All fields have been resolved according to privacy settings,
/// deduplication rules, and user preferences before the backend
/// sees them.
#[derive(Debug, Clone)]
pub struct RenderedNotification {
    pub id: String,
    pub title: String,
    pub body: String,
    pub event_type: String,
    pub conversation_target: Option<NotificationActionTarget>,
    pub actions: Vec<NotificationAction>,
    pub group_key: Option<String>,
    pub priority: NotificationPriority,
}

/// An action that can be taken on a notification.
#[derive(Debug, Clone)]
pub enum NotificationAction {
    /// Open the conversation or screen.
    Open,
    /// Mark the conversation as read.
    MarkAsRead,
    /// Accept a friend request.
    Accept,
    /// Decline a friend request.
    Decline,
    /// Dismiss this notification.
    Dismiss,
}

impl std::fmt::Display for NotificationAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotificationAction::Open => write!(f, "open"),
            NotificationAction::MarkAsRead => write!(f, "mark_as_read"),
            NotificationAction::Accept => write!(f, "accept"),
            NotificationAction::Decline => write!(f, "decline"),
            NotificationAction::Dismiss => write!(f, "dismiss"),
        }
    }
}

/// Platform-neutral notification backend interface.
///
/// Every desktop platform that supports native notifications implements
/// this trait. The core notification service uses this interface and
/// never calls platform-specific APIs directly.
pub trait NotificationBackend: std::fmt::Debug {
    /// Show a new notification.
    fn show(&self, notification: &RenderedNotification);

    /// Update an existing notification (e.g., to group new messages).
    fn update(&self, notification: &RenderedNotification);

    /// Close/dismiss a notification by its ID.
    fn close(&self, notification_id: &str);

    /// Whether this backend is available on the current platform.
    fn is_available(&self) -> bool;

    /// Request permission to show notifications (no-op on platforms
    /// where permission is implicit).
    fn request_permission(&self);
}

/// A no-op backend that silently discards all notifications.
///
/// Used on unsupported platforms and in tests where no real desktop
/// notification server is available.
#[derive(Debug)]
pub struct NoopBackend;

impl NotificationBackend for NoopBackend {
    fn show(&self, notification: &RenderedNotification) {
        tracing::debug!(
            "[noop-backend] would show: {} — {}",
            notification.title,
            notification.body
        );
    }

    fn update(&self, notification: &RenderedNotification) {
        tracing::debug!(
            "[noop-backend] would update: {} — {}",
            notification.title,
            notification.body
        );
    }

    fn close(&self, notification_id: &str) {
        tracing::debug!("[noop-backend] would close: {notification_id}");
    }

    fn is_available(&self) -> bool {
        false
    }

    fn request_permission(&self) {
        // No-op: no permission needed on platforms where noop is used.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notification::event::{NotificationEvent, NotificationEventKind};

    #[test]
    fn test_noop_backend_does_not_crash() {
        let backend = NoopBackend;
        assert!(!backend.is_available());

        let event = NotificationEvent::new(
            NotificationEventKind::NewMessage,
            None,
            None,
            "Test",
            "Body",
            None,
        );

        let rendered = RenderedNotification {
            id: event.notification_id.clone(),
            title: event.title_hint,
            body: event.body_hint,
            event_type: "NewMessage".to_string(),
            conversation_target: None,
            actions: vec![],
            group_key: event.group_key,
            priority: event.priority,
        };

        backend.show(&rendered);
        backend.update(&rendered);
        backend.close(&rendered.id);
        backend.request_permission();
    }

    #[test]
    fn test_notification_action_display() {
        assert_eq!(NotificationAction::Open.to_string(), "open");
        assert_eq!(NotificationAction::MarkAsRead.to_string(), "mark_as_read");
        assert_eq!(NotificationAction::Accept.to_string(), "accept");
        assert_eq!(NotificationAction::Decline.to_string(), "decline");
        assert_eq!(NotificationAction::Dismiss.to_string(), "dismiss");
    }
}
