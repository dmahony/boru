//! Thread projections and persistence helpers.
//!
//! Thread replies remain ordinary chat messages on the room topic.  The
//! optional root relation is only a projection hint: messages without it are
//! part of the main timeline, while replies can be rendered in a focused
//! thread without being duplicated in that timeline.

use n0_error::StdResultExt;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A stable content identifier for a chat message.
pub type MessageId = [u8; 32];

/// Wire/storage metadata used when a message is sent as a thread reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadTarget {
    /// The root message that owns the thread.
    pub thread_root_id: MessageId,
    /// An optional reply being directly addressed by this message.
    pub reply_to_message_id: Option<MessageId>,
}

impl ThreadTarget {
    /// Create a target for a direct reply to the root.
    pub const fn root(root: MessageId) -> Self {
        Self {
            thread_root_id: root,
            reply_to_message_id: None,
        }
    }

    /// Keep the root while addressing a specific reply.
    pub const fn reply(root: MessageId, reply_to: MessageId) -> Self {
        Self {
            thread_root_id: root,
            reply_to_message_id: Some(reply_to),
        }
    }
}

/// Select the root relation for a newly received message.
///
/// A missing root is intentionally preserved as `None`; this allows a late
/// root to be backfilled without hiding ordinary messages from the timeline.
pub const fn thread_root_for_message(target: Option<ThreadTarget>) -> Option<MessageId> {
    match target {
        Some(target) => Some(target.thread_root_id),
        None => None,
    }
}

/// Whether a stored message belongs in the main room timeline.
pub const fn is_main_timeline_message(thread_root_id: Option<MessageId>) -> bool {
    thread_root_id.is_none()
}

/// A thread summary that remains useful when the root has not arrived yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadSummary {
    /// Root message id.
    pub root_id: MessageId,
    /// Number of replies, excluding the root itself.
    pub reply_count: u64,
    /// Last message timestamp, including the root when present.
    pub last_activity_ms: u64,
    /// Whether the root is locally available.
    pub root_available: bool,
    /// Whether the root was explicitly deleted or tombstoned.
    pub root_deleted: bool,
}

impl ThreadSummary {
    /// Label suitable for a safe UI fallback when the root is unavailable.
    pub fn title(&self) -> String {
        if self.root_deleted {
            "Deleted message".to_owned()
        } else if !self.root_available {
            "Thread (root unavailable)".to_owned()
        } else {
            format!(
                "Thread ({} repl{})",
                self.reply_count,
                if self.reply_count == 1 { "y" } else { "ies" }
            )
        }
    }
}

/// Local follow/unread state for a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ThreadUnreadState {
    /// Whether this thread is followed for notifications.
    pub followed: bool,
    /// Number of replies received since the last read mark.
    pub unread_replies: u32,
}

/// Process-local unread tracker used by both narrow and wide thread views.
#[derive(Debug, Default, Clone)]
pub struct ThreadUnreadTracker {
    states: HashMap<MessageId, ThreadUnreadState>,
}

impl ThreadUnreadTracker {
    /// Set whether a thread is followed.
    pub fn set_followed(&mut self, root: MessageId, followed: bool) {
        self.states.entry(root).or_default().followed = followed;
    }

    /// Record a reply unless the focused thread is currently visible.
    pub fn record_reply(&mut self, root: MessageId, visible: bool) {
        if !visible {
            self.states.entry(root).or_default().unread_replies = self
                .states
                .get(&root)
                .map_or(1, |state| state.unread_replies.saturating_add(1));
        }
    }

    /// Clear unread state when the thread becomes visible.
    pub fn mark_read(&mut self, root: MessageId) {
        self.states.entry(root).or_default().unread_replies = 0;
    }

    /// Read the current state without creating one.
    pub fn state(&self, root: &MessageId) -> ThreadUnreadState {
        self.states.get(root).copied().unwrap_or_default()
    }
}

/// Central notification decision for thread replies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadNotificationPolicy {
    /// Whether desktop notifications are enabled.
    pub notifications_enabled: bool,
    /// Whether the application window is focused.
    pub window_focused: bool,
    /// Whether the target thread is the visible focused view.
    pub thread_visible: bool,
    /// Whether the user follows this thread.
    pub followed: bool,
}

impl ThreadNotificationPolicy {
    /// Decide whether a reply should produce a notification.
    pub const fn should_notify(self) -> bool {
        self.notifications_enabled
            && !self.thread_visible
            && (!self.window_focused || self.followed)
    }
}

/// Responsive layout mode for the focused thread view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadViewLayout {
    /// Thread pane alongside the room timeline.
    Wide,
    /// Thread pane takes the full content width.
    Narrow,
}

impl ThreadViewLayout {
    /// Resolve layout without allowing the composer to overflow a narrow window.
    pub const fn for_width(width: f32) -> Self {
        if width < 640.0 {
            Self::Narrow
        } else {
            Self::Wide
        }
    }
}

/// A persisted thread message projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredThreadMessage {
    /// Content hash/message id.
    pub message_id: MessageId,
    /// Room topic bytes.
    pub topic: [u8; 32],
    /// Sender public-key bytes.
    pub sender: [u8; 32],
    /// Timestamp in Unix milliseconds.
    pub timestamp_ms: u64,
    /// Signed message payload.
    pub signed_bytes: Vec<u8>,
    /// Optional thread root relation.
    pub thread_root_id: Option<MessageId>,
    /// Optional direct reply target.
    pub reply_to_message_id: Option<MessageId>,
    /// Tombstone marker for the root.
    pub deleted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_are_filtered_from_main_timeline() {
        assert!(is_main_timeline_message(None));
        assert!(!is_main_timeline_message(Some([7; 32])));
    }

    #[test]
    fn late_root_has_safe_summary_title() {
        let summary = ThreadSummary {
            root_id: [1; 32],
            reply_count: 2,
            last_activity_ms: 4,
            root_available: false,
            root_deleted: false,
        };
        assert_eq!(summary.title(), "Thread (root unavailable)");
    }

    #[test]
    fn unread_and_notification_policy_follow_visibility() {
        let mut tracker = ThreadUnreadTracker::default();
        tracker.record_reply([2; 32], false);
        assert_eq!(tracker.state(&[2; 32]).unread_replies, 1);
        assert!(ThreadNotificationPolicy {
            notifications_enabled: true,
            window_focused: false,
            thread_visible: false,
            followed: false
        }
        .should_notify());
        tracker.mark_read([2; 32]);
        assert_eq!(tracker.state(&[2; 32]).unread_replies, 0);
    }

    #[cfg(feature = "net")]
    #[test]
    fn sqlite_projection_keeps_late_root_replies_and_unread_state() {
        let storage = crate::storage::Storage::memory().unwrap();
        let topic = [3; 32];
        let root = [4; 32];
        let reply = [5; 32];
        storage
            .insert_thread_message(&root, &topic, &[6; 32], 10, b"root", None)
            .unwrap();
        storage
            .insert_thread_message(
                &reply,
                &topic,
                &[7; 32],
                20,
                b"reply",
                Some(ThreadTarget::root(root)),
            )
            .unwrap();
        storage.record_thread_reply(&topic, &root, false).unwrap();
        let rows = storage
            .list_thread_messages_for_topic(&topic, Some(&root), false)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message_id, reply);
        assert_eq!(storage.thread_summary(&topic, &root).unwrap().reply_count, 1);
        assert_eq!(storage.thread_unread_state(&topic, &root).unwrap().unread_replies, 1);
    }

    #[cfg(feature = "net")]
    #[test]
    fn thread_message_roundtrips_on_wire() {
        let target = ThreadTarget::reply([8; 32], [9; 32]);
        let message = crate::chat_core::Message::ThreadMessage {
            text: "reply".to_string(),
            target,
        };
        let encoded = postcard::to_stdvec(&message).unwrap();
        let decoded: crate::chat_core::Message = postcard::from_bytes(&encoded).unwrap();
        assert!(matches!(decoded, crate::chat_core::Message::ThreadMessage { text, target: got } if text == "reply" && got == target));
    }
}

impl crate::storage::Storage {
    /// Insert a chat message with optional thread targeting metadata.
    pub fn insert_thread_message(
        &self,
        message_id: &MessageId,
        topic: &[u8; 32],
        sender: &[u8; 32],
        timestamp_ms: u64,
        signed_bytes: &[u8],
        target: Option<ThreadTarget>,
    ) -> n0_error::Result<bool> {
        self.with_conn(|conn| {
            let (root, reply) = target.map_or((None, None), |target| (Some(target.thread_root_id.to_vec()), target.reply_to_message_id.map(|id| id.to_vec())));
            let rows = conn.execute(
                "INSERT OR IGNORE INTO chat_messages (msg_hash, topic, sender, timestamp_ms, signed_bytes, thread_root_id, reply_to_message_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![message_id.as_slice(), topic.as_slice(), sender.as_slice(), timestamp_ms as i64, signed_bytes, root, reply],
            ).std_context("insert thread message")?;
            Ok(rows > 0)
        })
    }

    /// List room messages, optionally excluding thread replies from the main timeline.
    pub fn list_thread_messages_for_topic(
        &self,
        topic: &[u8; 32],
        root: Option<&MessageId>,
        main_timeline: bool,
    ) -> n0_error::Result<Vec<StoredThreadMessage>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT msg_hash, topic, sender, timestamp_ms, signed_bytes, thread_root_id, reply_to_message_id, deleted FROM chat_messages WHERE topic = ?1 AND ((?2 = 1 AND thread_root_id IS NULL) OR (?2 = 0 AND (?3 IS NULL OR thread_root_id = ?3))) ORDER BY timestamp_ms ASC, id ASC").std_context("prepare thread messages")?;
            let root_bytes = root.map(|id| id.as_slice());
            let rows = stmt.query_map(rusqlite::params![topic.as_slice(), main_timeline as i64, root_bytes], |row| {
                let parse_id = |index| -> rusqlite::Result<Option<MessageId>> { let bytes: Option<Vec<u8>> = row.get(index)?; bytes.map(|v| v.try_into().map_err(|_| rusqlite::Error::InvalidQuery)).transpose() };
                Ok(StoredThreadMessage { message_id: row.get::<_, Vec<u8>>(0)?.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?, topic: row.get::<_, Vec<u8>>(1)?.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?, sender: row.get::<_, Vec<u8>>(2)?.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?, timestamp_ms: row.get::<_, i64>(3)? as u64, signed_bytes: row.get(4)?, thread_root_id: parse_id(5)?, reply_to_message_id: parse_id(6)?, deleted: row.get::<_, i64>(7)? != 0 })
            }).std_context("query thread messages")?;
            let mut result = Vec::new();
            for row in rows {
                result.push(row.std_context("read thread message")?);
            }
            Ok(result)
        })
    }

    /// Return a summary even when the root is missing or tombstoned.
    pub fn thread_summary(
        &self,
        topic: &[u8; 32],
        root: &MessageId,
    ) -> n0_error::Result<ThreadSummary> {
        self.with_conn(|conn| {
            let root_bytes = root.as_slice();
            let (count, last): (i64, i64) = conn.query_row("SELECT COUNT(*), COALESCE(MAX(timestamp_ms), 0) FROM chat_messages WHERE topic = ?1 AND thread_root_id = ?2", rusqlite::params![topic.as_slice(), root_bytes], |row| Ok((row.get(0)?, row.get(1)?))).std_context("summarize thread replies")?;
            let root_row: Option<(bool, bool, i64)> = conn.query_row("SELECT deleted, 1, timestamp_ms FROM chat_messages WHERE topic = ?1 AND msg_hash = ?2", rusqlite::params![topic.as_slice(), root_bytes], |row| Ok((row.get::<_, i64>(0)? != 0, true, row.get(2)?))).optional().std_context("look up thread root")?;
            let (deleted, available, root_ts) = root_row.unwrap_or((false, false, 0));
            Ok(ThreadSummary { root_id: *root, reply_count: count as u64, last_activity_ms: (last as u64).max(root_ts as u64), root_available: available, root_deleted: deleted })
        })
    }

    /// Tombstone a root without deleting replies or their searchable metadata.
    pub fn mark_thread_root_deleted(
        &self,
        topic: &[u8; 32],
        root: &MessageId,
    ) -> n0_error::Result<bool> {
        self.with_conn(|conn| {
            Ok(conn
                .execute(
                    "UPDATE chat_messages SET deleted = 1 WHERE topic = ?1 AND msg_hash = ?2",
                    rusqlite::params![topic.as_slice(), root.as_slice()],
                )
                .std_context("tombstone thread root")?
                > 0)
        })
    }

    /// Persist whether a user follows a thread for notification purposes.
    pub fn set_thread_followed(
        &self,
        topic: &[u8; 32],
        root: &MessageId,
        followed: bool,
    ) -> n0_error::Result<()> {
        self.with_conn(|conn| {
            conn.execute("INSERT INTO thread_state (topic, thread_root_id, followed) VALUES (?1, ?2, ?3) ON CONFLICT(topic, thread_root_id) DO UPDATE SET followed = excluded.followed", rusqlite::params![topic.as_slice(), root.as_slice(), followed as i64]).std_context("set thread follow state")?;
            Ok(())
        })
    }

    /// Read the durable follow/unread projection for a thread.
    pub fn thread_unread_state(
        &self,
        topic: &[u8; 32],
        root: &MessageId,
    ) -> n0_error::Result<ThreadUnreadState> {
        self.with_conn(|conn| {
            let state = conn.query_row("SELECT followed, unread_replies FROM thread_state WHERE topic = ?1 AND thread_root_id = ?2", rusqlite::params![topic.as_slice(), root.as_slice()], |row| Ok(ThreadUnreadState { followed: row.get::<_, i64>(0)? != 0, unread_replies: row.get::<_, i64>(1)? as u32 })).optional().std_context("get thread unread state")?;
            Ok(state.unwrap_or_default())
        })
    }

    /// Increment unread replies for a received reply unless the caller has
    /// already determined that the focused thread is visible.
    pub fn record_thread_reply(
        &self,
        topic: &[u8; 32],
        root: &MessageId,
        visible: bool,
    ) -> n0_error::Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO thread_state (topic, thread_root_id, unread_replies) VALUES (?1, ?2, ?3)
                 ON CONFLICT(topic, thread_root_id) DO UPDATE SET
                   unread_replies = thread_state.unread_replies + excluded.unread_replies",
                rusqlite::params![topic.as_slice(), root.as_slice(), (!visible) as i64],
            )
            .std_context("record thread reply")?;
            Ok(())
        })
    }

    /// Mark a focused thread read and reset its unread count.
    pub fn mark_thread_read(
        &self,
        topic: &[u8; 32],
        root: &MessageId,
        read_at_ms: u64,
    ) -> n0_error::Result<()> {
        self.with_conn(|conn| {
            conn.execute("INSERT INTO thread_state (topic, thread_root_id, read_at_ms) VALUES (?1, ?2, ?3) ON CONFLICT(topic, thread_root_id) DO UPDATE SET unread_replies = 0, read_at_ms = excluded.read_at_ms", rusqlite::params![topic.as_slice(), root.as_slice(), read_at_ms as i64]).std_context("mark thread read")?;
            Ok(())
        })
    }
}
