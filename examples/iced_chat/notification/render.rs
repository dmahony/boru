//! Notification rendering — converts internal events into displayable titles
//! and bodies according to privacy settings and event type.
//!
//! The renderer is separate from the service so it can be unit-tested
//! independently and reused by multiple frontends.

use crate::notification::event::{
    NotificationEvent, NotificationEventKind,
};

/// Privacy mode for notification content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewMode {
    /// Show sender display name and message preview.
    Full,
    /// Show only the sender's display name.
    SenderOnly,
    /// Hide both sender and message content.
    Hidden,
}

impl PreviewMode {
    /// Parse from a string (e.g. from settings).
    pub fn from_str(s: &str) -> Self {
        match s {
            "sender_only" => Self::SenderOnly,
            "hidden" => Self::Hidden,
            _ => Self::Full,
        }
    }

    /// Serialize to a string for persistence.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::SenderOnly => "sender_only",
            Self::Hidden => "hidden",
        }
    }
}

/// Rendered notification content — the final title and body shown to the user.
#[derive(Debug, Clone)]
pub struct RenderedNotification {
    /// The stable notification ID (from the source event).
    pub id: String,
    /// The rendered notification title (e.g. sender name or app name).
    pub title: String,
    /// The rendered notification body (e.g. message preview or event summary).
    pub body: String,
    /// The event kind label (for backend categorisation).
    pub event_kind: String,
    /// Group key for coalescing related notifications.
    pub group_key: Option<String>,
}

/// Render a notification event into a displayable form according to the
/// current privacy mode.
pub fn render_event(event: &NotificationEvent, preview_mode: PreviewMode) -> RenderedNotification {
    let (title, body) = match preview_mode {
        PreviewMode::Hidden => {
            ("Boru Chat".to_string(), render_hidden_body(event.event_kind))
        }
        PreviewMode::SenderOnly => {
            let title = event.title_hint.clone();
            (title, render_sender_only_body(event.event_kind))
        }
        PreviewMode::Full => {
            let title = event.title_hint.clone();
            let body = sanitize_preview(&event.body_hint);
            (title, body)
        }
    };

    RenderedNotification {
        id: event.notification_id.clone(),
        title,
        body,
        event_kind: format!("{:?}", event.event_kind),
        group_key: event.group_key.clone(),
    }
}

/// Render the body text for hidden preview mode.
fn render_hidden_body(kind: NotificationEventKind) -> String {
    match kind {
        NotificationEventKind::NewMessage => "New message".into(),
        NotificationEventKind::FriendRequest => "New friend request".into(),
        NotificationEventKind::FriendRequestAccepted => "Friend request accepted".into(),
        NotificationEventKind::FileTransferCompleted => "File transfer completed".into(),
        NotificationEventKind::FileTransferFailed => "File transfer failed".into(),
        NotificationEventKind::ConnectionLost => "is offline".into(),
        NotificationEventKind::ConnectionRestored => "is online again".into(),
    }
}

/// Render the body text for sender-only preview mode.
fn render_sender_only_body(kind: NotificationEventKind) -> String {
    match kind {
        NotificationEventKind::NewMessage => "New message".into(),
        NotificationEventKind::FriendRequest => "wants to connect".into(),
        NotificationEventKind::FriendRequestAccepted => "accepted your request".into(),
        NotificationEventKind::FileTransferCompleted => "sent a file".into(),
        NotificationEventKind::FileTransferFailed => "file transfer failed".into(),
        NotificationEventKind::ConnectionLost => "is offline".into(),
        NotificationEventKind::ConnectionRestored => "is online again".into(),
    }
}

/// Sanitize a notification preview string.
///
/// - Removes control characters (except newlines)
/// - Collapses excessive whitespace
/// - Truncates to a maximum length
pub fn sanitize_preview(text: &str) -> String {
    const MAX_PREVIEW_LEN: usize = 200;

    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect();

    // Collapse excessive whitespace
    let mut result = String::with_capacity(cleaned.len().min(MAX_PREVIEW_LEN + 20));
    let mut prev_was_space = false;
    for ch in cleaned.chars() {
        if ch.is_whitespace() && ch != '\n' {
            if prev_was_space {
                continue;
            }
            prev_was_space = true;
            result.push(' ');
        } else {
            prev_was_space = false;
            result.push(ch);
        }
    }

    let result = result.trim();
    if result.len() > MAX_PREVIEW_LEN {
        format!("{}…", &result[..MAX_PREVIEW_LEN])
    } else {
        result.to_string()
    }
}

/// Render a group summary notification (e.g. "Alice\n4 new messages").
pub fn render_group_summary(
    title: &str,
    count: usize,
    conversations: usize,
) -> (String, String) {
    if count <= 1 {
        (title.to_string(), String::new())
    } else if conversations <= 1 {
        // Single conversation
        (title.to_string(), format!("{} new messages", count))
    } else {
        // Multiple conversations
        (
            "Boru Chat".to_string(),
            format!("{} new messages from {} conversations", count, conversations),
        )
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notification::event::NotificationEvent;
    use std::str::FromStr;

    fn test_peer() -> iroh::PublicKey {
        iroh::PublicKey::from_str("z3emgk36rht3zv2k4mnu6gh5yu2bhvwz7xkemhl6ckqxw3vgvnga").unwrap()
    }

    fn make_text_event(body: &str) -> NotificationEvent {
        NotificationEvent::new(
            NotificationEventKind::NewMessage,
            Some(test_peer()),
            None,
            "Alice",
            body,
            None,
        )
    }

    #[test]
    fn test_full_preview() {
        let event = make_text_event("Hey, are you coming?");
        let rendered = render_event(&event, PreviewMode::Full);
        assert_eq!(rendered.title, "Alice");
        assert_eq!(rendered.body, "Hey, are you coming?");
    }

    #[test]
    fn test_sender_only_preview() {
        let event = make_text_event("Secret details here");
        let rendered = render_event(&event, PreviewMode::SenderOnly);
        assert_eq!(rendered.title, "Alice");
        assert_eq!(rendered.body, "New message");
    }

    #[test]
    fn test_hidden_preview() {
        let event = make_text_event("Secret details here");
        let rendered = render_event(&event, PreviewMode::Hidden);
        assert_eq!(rendered.title, "Boru Chat");
        assert_eq!(rendered.body, "New message");
    }

    #[test]
    fn test_friend_request_full() {
        let event = NotificationEvent::new(
            NotificationEventKind::FriendRequest,
            Some(test_peer()),
            None,
            "Bob",
            "wants to connect",
            None,
        );
        let rendered = render_event(&event, PreviewMode::Full);
        assert_eq!(rendered.title, "Bob");
        assert_eq!(rendered.body, "wants to connect");
    }

    #[test]
    fn test_friend_request_hidden() {
        let event = NotificationEvent::new(
            NotificationEventKind::FriendRequest,
            Some(test_peer()),
            None,
            "Bob",
            "wants to connect",
            None,
        );
        let rendered = render_event(&event, PreviewMode::Hidden);
        assert_eq!(rendered.title, "Boru Chat");
        assert_eq!(rendered.body, "New friend request");
    }

    #[test]
    fn test_file_transfer_full() {
        let event = NotificationEvent::new(
            NotificationEventKind::FileTransferCompleted,
            None,
            None,
            "Boru Chat",
            "photo.jpg received",
            None,
        );
        let rendered = render_event(&event, PreviewMode::Full);
        assert_eq!(rendered.title, "Boru Chat");
        assert_eq!(rendered.body, "photo.jpg received");
    }

    #[test]
    fn test_sanitize_preview_truncates() {
        let long = "a".repeat(300);
        let result = sanitize_preview(&long);
        assert!(result.len() <= 201);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_sanitize_preview_control_chars() {
        let dirty = "Hello\x00\x01\x02World";
        assert_eq!(sanitize_preview(dirty), "HelloWorld");
    }

    #[test]
    fn test_sanitize_preview_whitespace() {
        let spaced = "Hello    World";
        assert_eq!(sanitize_preview(spaced), "Hello World");
    }

    #[test]
    fn test_render_group_summary_single() {
        let (title, body) = render_group_summary("Alice", 1, 1);
        assert_eq!(title, "Alice");
        assert!(body.is_empty());
    }

    #[test]
    fn test_render_group_summary_multiple_messages() {
        let (title, body) = render_group_summary("Alice", 4, 1);
        assert_eq!(title, "Alice");
        assert_eq!(body, "4 new messages");
    }

    #[test]
    fn test_render_group_summary_multi_conv() {
        let (title, body) = render_group_summary("Alice", 7, 3);
        assert_eq!(title, "Boru Chat");
        assert_eq!(body, "7 new messages from 3 conversations");
    }

    #[test]
    fn test_connection_events_hidden() {
        let event = NotificationEvent::new(
            NotificationEventKind::ConnectionLost,
            None,
            None,
            "Boru Chat",
            "is offline",
            None,
        );
        let rendered = render_event(&event, PreviewMode::Hidden);
        assert_eq!(rendered.title, "Boru Chat");
        assert_eq!(rendered.body, "is offline");

        let event = NotificationEvent::new(
            NotificationEventKind::ConnectionRestored,
            None,
            None,
            "Boru Chat",
            "is online again",
            None,
        );
        let rendered = render_event(&event, PreviewMode::Hidden);
        assert_eq!(rendered.title, "Boru Chat");
        assert_eq!(rendered.body, "is online again");
    }
}
