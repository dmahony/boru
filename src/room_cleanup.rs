//! Room cleanup helpers for deleting all local state associated with a room.
//!
//! The helpers here are intentionally server-side / backend-side: they operate
//! on the durable stores and in-memory room lists without touching frontend UI
//! state.

use std::path::Path;

use n0_error::Result;

use crate::{
    chat_history::ChatHistoryStore, friends::FriendsStore, outbox::OutboxStore, proto::TopicId,
    room::RoomStore, room_history::RoomHistoryStore,
};

/// Summary of a room-history deletion operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoomHistoryDeletionReport {
    /// The room topic that was purged.
    pub topic: TopicId,
    /// Whether the in-memory room history list contained the room.
    pub room_history_removed: bool,
    /// Number of chat history entries removed for this topic.
    pub chat_entries_removed: usize,
    /// Number of outbox entries removed for this topic.
    pub outbox_entries_removed: usize,
    /// Number of friend records whose room metadata changed.
    pub friend_records_updated: usize,
    /// Whether the persisted active-room file was removed.
    pub room_file_removed: bool,
    /// Whether the legacy `rooms.json` file was removed.
    pub legacy_room_history_file_removed: bool,
}

impl RoomHistoryDeletionReport {
    fn new(topic: TopicId) -> Self {
        Self {
            topic,
            room_history_removed: false,
            chat_entries_removed: 0,
            outbox_entries_removed: 0,
            friend_records_updated: 0,
            room_file_removed: false,
            legacy_room_history_file_removed: false,
        }
    }
}

/// Delete all local history and metadata associated with a room topic.
///
/// The function is idempotent: repeated calls for the same room safely return
/// a report with zero removals once the room has already been purged.
pub fn delete_room_history(
    data_dir: impl AsRef<Path>,
    topic: TopicId,
    room_history: &mut RoomHistoryStore,
    chat_history: &mut ChatHistoryStore,
    outbox: Option<&mut OutboxStore>,
    friends: Option<&mut FriendsStore>,
) -> Result<RoomHistoryDeletionReport> {
    let data_dir = data_dir.as_ref();
    let mut report = RoomHistoryDeletionReport::new(topic);

    report.room_history_removed = room_history.remove(&topic);
    report.chat_entries_removed = chat_history.remove_topic(&topic);
    report.outbox_entries_removed = outbox.map_or(0, |store| store.remove_topic(&topic));
    report.friend_records_updated = friends.map_or(0, |store| store.remove_room(&topic));

    report.legacy_room_history_file_removed = RoomHistoryStore::delete_legacy_file(data_dir)?;
    report.room_file_removed = match RoomStore::load_or_none(data_dir) {
        Some(room) if room.topic == topic => RoomStore::delete(data_dir)?,
        _ => false,
    };

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        chat_core::Ticket,
        chat_history::{ChatHistoryStore, HistoryEntry},
        friends::{DirectConversationState, FriendId, FriendRecord},
        outbox::OutboxEntry,
        proto::TopicId,
        room::{RoomStore, ROOM_FILE_NAME},
        room_history::{RoomHistoryStore, ROOM_HISTORY_FILE_NAME},
    };
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        dir.push(format!("boru-room-cleanup-{name}-{suffix}"));
        dir
    }

    fn topic(byte: u8) -> TopicId {
        TopicId::from_bytes([byte; 32])
    }

    fn history_entry(topic: TopicId, label: &str) -> HistoryEntry {
        HistoryEntry::new(topic, "sender", Vec::new(), "text", label)
    }

    fn outbox_entry(event_id: u64, topic: TopicId) -> OutboxEntry {
        OutboxEntry::new(event_id, topic, format!("bytes-{event_id}").into_bytes())
    }

    fn friend_record(topic: TopicId) -> FriendRecord {
        let mut record = FriendRecord {
            direct_conversation: Some(crate::friends::DirectConversation {
                topic,
                state: DirectConversationState::Active,
            }),
            ..Default::default()
        };
        record.rooms.insert(
            topic,
            Ticket {
                topic,
                peers: Vec::new(),
            },
        );
        record
    }

    #[test]
    fn delete_room_history_cascades_across_stores() {
        let dir = temp_dir("cascade");
        fs::create_dir_all(&dir).unwrap();

        let target = topic(0xAA);
        let other = topic(0xBB);

        // Saved active-room file for the target room.
        RoomStore::new(&dir, target).save().unwrap();
        // Legacy history file should be removed too.
        fs::write(dir.join(ROOM_HISTORY_FILE_NAME), b"legacy rooms").unwrap();
        // Create a placeholder file to ensure we only remove the matching room file.
        let other_room_path = dir.join(ROOM_FILE_NAME);
        assert!(other_room_path.exists());

        let mut room_history = RoomHistoryStore::empty_at(&dir);
        room_history.upsert(target, "Target", true);
        room_history.upsert(other, "Other", false);

        let mut chat_history = ChatHistoryStore::empty_at(&dir);
        chat_history.push(history_entry(target, "target-1"));
        chat_history.push(history_entry(other, "other-1"));
        chat_history.push(history_entry(target, "target-2"));

        let mut outbox = OutboxStore::empty_at(&dir);
        outbox.push(outbox_entry(1, target)).unwrap();
        outbox.push(outbox_entry(2, other)).unwrap();
        outbox.push(outbox_entry(3, target)).unwrap();

        let mut friends = FriendsStore::empty_at(&dir);
        let friend_id = FriendId::new("friend-1");
        friends.upsert(friend_id, friend_record(target));
        let other_friend_id = FriendId::new("friend-2");
        let mut other_friend = friend_record(other);
        other_friend.rooms.insert(
            target,
            Ticket {
                topic: target,
                peers: Vec::new(),
            },
        );
        friends.upsert(other_friend_id, other_friend);

        let report = delete_room_history(
            &dir,
            target,
            &mut room_history,
            &mut chat_history,
            Some(&mut outbox),
            Some(&mut friends),
        )
        .unwrap();

        assert_eq!(report.topic, target);
        assert!(report.room_history_removed);
        assert_eq!(report.chat_entries_removed, 2);
        assert_eq!(report.outbox_entries_removed, 2);
        assert_eq!(report.friend_records_updated, 2);
        assert!(report.room_file_removed);
        assert!(report.legacy_room_history_file_removed);

        assert!(room_history.find(&target).is_none());
        assert!(room_history.find(&other).is_some());
        assert_eq!(
            chat_history
                .entries()
                .iter()
                .filter(|e| e.topic == target)
                .count(),
            0
        );
        assert_eq!(
            chat_history
                .entries()
                .iter()
                .filter(|e| e.topic == other)
                .count(),
            1
        );
        assert_eq!(
            outbox
                .entries()
                .iter()
                .filter(|e| e.topic == target)
                .count(),
            0
        );
        assert_eq!(
            outbox.entries().iter().filter(|e| e.topic == other).count(),
            1
        );
        assert!(friends
            .get(&FriendId::new("friend-1"))
            .unwrap()
            .rooms
            .get(&target)
            .is_none());
        assert!(friends
            .get(&FriendId::new("friend-1"))
            .unwrap()
            .direct_conversation()
            .is_none());
        assert!(friends
            .get(&FriendId::new("friend-2"))
            .unwrap()
            .rooms
            .get(&target)
            .is_none());
        assert!(friends
            .get(&FriendId::new("friend-2"))
            .unwrap()
            .rooms
            .get(&other)
            .is_some());
        assert!(friends
            .get(&FriendId::new("friend-2"))
            .unwrap()
            .direct_conversation()
            .is_some());
        assert!(!dir.join(ROOM_FILE_NAME).exists());
        assert!(!dir.join(ROOM_HISTORY_FILE_NAME).exists());
    }

    #[test]
    fn delete_room_history_is_idempotent() {
        let dir = temp_dir("idempotent");
        fs::create_dir_all(&dir).unwrap();
        let target = topic(0xCC);
        let mut room_history = RoomHistoryStore::empty_at(&dir);
        let mut chat_history = ChatHistoryStore::empty_at(&dir);
        let mut outbox = OutboxStore::empty_at(&dir);
        let mut friends = FriendsStore::empty_at(&dir);

        let first = delete_room_history(
            &dir,
            target,
            &mut room_history,
            &mut chat_history,
            Some(&mut outbox),
            Some(&mut friends),
        )
        .unwrap();
        let second = delete_room_history(
            &dir,
            target,
            &mut room_history,
            &mut chat_history,
            Some(&mut outbox),
            Some(&mut friends),
        )
        .unwrap();

        assert_eq!(first.room_history_removed, false);
        assert_eq!(second.room_history_removed, false);
        assert_eq!(second.chat_entries_removed, 0);
        assert_eq!(second.outbox_entries_removed, 0);
        assert_eq!(second.friend_records_updated, 0);
    }
}
