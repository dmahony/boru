//! Pure notification and unread/read policy shared by desktop adapters.
//!
//! `NotificationPayload` is the stable adapter/deep-link contract: identifiers
//! are opaque strings and display fields are already privacy-filtered. Adapters
//! must use `build_payload` rather than forwarding message bodies themselves.

use serde::{Deserialize, Serialize};

/// Inputs to the notification decision. `conversation_visible` represents the
/// focused conversation, while `window_visible` includes focused or visible app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationContext {
    /// Master notification preference.
    pub enabled: bool,
    /// Whether this specific message notification category is enabled.
    pub messages_enabled: bool,
    /// Whether app window is focused or visible.
    pub window_visible: bool,
    /// Whether policy allows alerts while the window is visible.
    pub notify_while_visible: bool,
    /// Whether the destination conversation is actively visible.
    pub conversation_visible: bool,
    /// Whether the conversation is currently muted.
    pub muted: bool,
    /// Whether this conversation is followed.
    pub followed: bool,
}

/// Decide whether to emit a message notification.
///
/// Matches desktop semantics: focused/visible apps suppress notifications unless
/// explicitly enabled; an actively visible destination is never notified. Follow
/// permits a background notification but does not override mute or visibility.
pub const fn should_emit_notification(context: NotificationContext) -> bool {
    context.enabled
        && context.messages_enabled
        && !context.muted
        && !context.conversation_visible
        && (!context.window_visible || context.notify_while_visible)
}

/// Return whether message text is safe to include in a notification.
pub const fn should_include_body(privacy_mode: bool) -> bool {
    !privacy_mode
}

/// Return the current mute state (expired mutes must be resolved by caller).
pub const fn is_conversation_muted(muted: bool) -> bool { muted }

/// Return whether the conversation is followed.
pub const fn is_conversation_followed(followed: bool) -> bool { followed }

/// Stable, privacy-filtered notification data for native adapters and deep links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPayload {
    /// Opaque conversation identifier used to navigate to the conversation.
    pub conversation_id: String,
    /// Opaque message/item identifier to focus after navigation.
    pub item_id: String,
    /// Stable identifier for deduplication and native notification actions.
    pub notification_id: String,
    /// Safe sender display label, or generic label in privacy mode.
    pub title: String,
    /// Display body; never contains message text when privacy mode is enabled.
    pub body: String,
    /// True only when the body contains permitted message text.
    pub includes_message_body: bool,
}

/// Construct a safe payload. Privacy mode replaces both title and body so
/// message-derived text cannot leak through native notification surfaces.
pub fn build_payload(
    conversation_id: impl Into<String>, item_id: impl Into<String>,
    notification_id: impl Into<String>, sender: &str, message_body: &str,
    privacy_mode: bool,
) -> NotificationPayload {
    let (title, body, includes_message_body) = if privacy_mode {
        ("Boru Chat".to_owned(), "New message".to_owned(), false)
    } else {
        (sender.to_owned(), message_body.to_owned(), true)
    };
    NotificationPayload {
        conversation_id: conversation_id.into(), item_id: item_id.into(),
        notification_id: notification_id.into(), title, body, includes_message_body,
    }
}

/// Action that can legitimately transition an unread item to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadAction {
    /// The user explicitly opened or acknowledged the item.
    Explicit,
    /// The item is actually visible in the active conversation.
    Visible,
    /// Background receipt, notification delivery, or other non-user event.
    Background,
}

/// Whether an unread item may transition to read after this action.
pub const fn may_mark_read(is_unread: bool, action: ReadAction) -> bool {
    is_unread && !matches!(action, ReadAction::Background)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> NotificationContext {
        NotificationContext { enabled: true, messages_enabled: true, window_visible: false,
            notify_while_visible: false, conversation_visible: false, muted: false, followed: false }
    }
    #[test]
    fn foreground_background_mute_follow_matrix() {
        let mut c = context();
        assert!(should_emit_notification(c));
        c.window_visible = true;
        assert!(!should_emit_notification(c));
        c.notify_while_visible = true;
        assert!(should_emit_notification(c));
        c.conversation_visible = true;
        assert!(!should_emit_notification(c));
        c.conversation_visible = false; c.notify_while_visible = false; c.window_visible = false; c.muted = true;
        assert!(!should_emit_notification(c));
        c.muted = false; c.followed = true;
        assert!(is_conversation_followed(c.followed));
        assert!(should_emit_notification(c));
        assert!(is_conversation_muted(true));
    }
    #[test]
    fn privacy_payload_never_contains_sensitive_body() {
        let safe = build_payload("c", "i", "n", "Alice", "secret text", true);
        assert_eq!(safe.body, "New message");
        assert_eq!(safe.title, "Boru Chat");
        assert!(!safe.includes_message_body);
        let full = build_payload("c", "i", "n", "Alice", "hello", false);
        assert_eq!(full.body, "hello");
        assert!(full.includes_message_body);
    }
    #[test]
    fn unread_requires_explicit_or_visible_action() {
        assert!(!may_mark_read(true, ReadAction::Background));
        assert!(may_mark_read(true, ReadAction::Visible));
        assert!(may_mark_read(true, ReadAction::Explicit));
        assert!(!may_mark_read(false, ReadAction::Explicit));
    }
}
