//! Startup migration that removes the stale saved lobby conversation.
//!
//! Older Boru versions auto-joined the canonical public lobby on startup and
//! persisted it like any other room: a
//! [`ConversationEntry`](crate::conversations::ConversationEntry) in the
//! conversation store (`conversations.json` / the SQLite `kv_store`
//! `"conversations"` key) and per-topic message history in the SQLite
//! `messages` table (`message_store.db`). Since BORU-DISC-12 the lobby is
//! never auto-joined and never shown in the UI, but an install upgraded from
//! an old version can still carry a stale persisted lobby entry and its
//! message history.
//!
//! This module detects those entries at startup and removes them — WITHOUT
//! touching unrelated public rooms. Only the exact canonical lobby topic
//! (`public_room_topic(0x00, "public-lobby", 1)`) is matched; every other
//! public room has a different topic and is left intact.
//!
//! Guardrail compliance (BORU-DISC-18 / PDF T16):
//! - Discovery state is never merged with conversation state: this migration
//!   only touches conversation persistence (room list + chat history). It
//!   does not create or alter any discovery object.
//! - Public chat creation/joining stays explicit: only the persisted legacy
//!   auto-lobby entry is removed; user-created public rooms keep their topic,
//!   metadata, and history.

use std::path::Path;

use tracing::{info, warn};

use crate::chat_history::ChatHistoryStore;
use crate::conversations::ConversationStore;
use crate::proto::TopicId;
use crate::public_room::{public_lobby_topic, PublicNetwork};
use crate::storage::Storage;
use crate::store::MessageStore;

/// The canonical legacy auto-lobby gossip topic.
///
/// Older versions auto-joined this topic on startup and persisted it as an
/// ordinary conversation. The canonical lobby identity is MUST-KEEP
/// (`docs/compatibility-identifiers.md`), so the migration matches this exact
/// topic and never any other.
pub fn legacy_lobby_topic() -> TopicId {
    public_lobby_topic(PublicNetwork::Mainnet)
}

/// Whether `topic` is the legacy auto-lobby topic.
///
/// This is the single filter used by every prune step. It matches exactly one
/// topic — the canonical mainnet public-lobby derivation — so unrelated public
/// rooms (different room names, networks, or versions) are never touched.
pub fn is_legacy_lobby_topic(topic: &TopicId) -> bool {
    *topic == legacy_lobby_topic()
}

/// What the startup migration removed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LobbyMigrationReport {
    /// Conversation (room-list) entries removed.
    pub conversations_removed: usize,
    /// Chat messages removed from the SQLite `messages` table.
    pub messages_removed: usize,
    /// `conversation_meta` sidebar rows removed for the lobby topic.
    pub meta_rows_removed: usize,
}

impl LobbyMigrationReport {
    /// Whether the migration removed anything.
    pub fn is_empty(self) -> bool {
        self.conversations_removed == 0 && self.messages_removed == 0 && self.meta_rows_removed == 0
    }
}

/// Remove legacy-lobby entries from an in-memory conversation store.
///
/// Returns the number of entries removed. Entries for any other topic are
/// left untouched.
pub fn prune_conversation_store(store: &mut ConversationStore) -> usize {
    let topics: Vec<TopicId> = store
        .iter()
        .filter(|entry| is_legacy_lobby_topic(&entry.topic))
        .map(|entry| entry.topic)
        .collect();
    let removed = topics.len();
    for topic in topics {
        store.remove(&topic);
    }
    removed
}

/// Remove legacy-lobby entries from an in-memory chat-history store.
///
/// Returns the number of entries removed. Entries for any other topic are
/// left untouched.
pub fn prune_chat_history(store: &mut ChatHistoryStore) -> usize {
    let before = store.entries.len();
    store
        .entries
        .retain(|entry| !is_legacy_lobby_topic(&entry.topic));
    before - store.entries.len()
}

/// Prune the persisted conversation store (SQLite `kv_store` and the legacy
/// `conversations.json` fallback). Returns the number of entries removed.
fn prune_persisted_conversations(data_dir: &Path, storage: Option<&Storage>) -> usize {
    let mut removed = 0;

    // Primary: SQLite kv_store ("conversations" key).
    if let Some(st) = storage {
        let mut store = ConversationStore::load_from_sqlite(st, data_dir);
        let n = prune_conversation_store(&mut store);
        if n > 0 {
            match store.save_to_sqlite(st) {
                Ok(()) => removed += n,
                Err(err) => warn!(
                    err = %err,
                    "lobby migration: failed to persist pruned conversation store to SQLite"
                ),
            }
        }
    }

    // Fallback: legacy conversations.json (still read when SQLite has no
    // store). The file is never written back — SQLite is the single source
    // of truth, so the legacy JSON is only a one-time migration input, not
    // a store to keep in sync.
    let json_path = data_dir.join(crate::conversations::CONVERSATIONS_FILE_NAME);
    if json_path.exists() {
        let mut store = ConversationStore::load_or_default(data_dir);
        let n = prune_conversation_store(&mut store);
        if n > 0 {
            removed += n;
        }
    }

    removed
}

/// Prune the lobby topic from the SQLite chat-message store
/// (`message_store.db`): the `messages` table and the `conversation_meta`
/// sidebar rows it feeds. Returns `(messages_removed, meta_rows_removed)`.
fn prune_persisted_messages(data_dir: &Path) -> (usize, usize) {
    let ms_path = data_dir.join("message_store.db");
    if !ms_path.exists() {
        return (0, 0);
    }
    let ms = match MessageStore::open(&ms_path) {
        Ok(ms) => ms,
        Err(err) => {
            warn!(err = %err, "lobby migration: failed to open message store");
            return (0, 0);
        }
    };
    let lobby = legacy_lobby_topic();
    let messages_removed = match ms.delete_messages_for_topic(lobby.as_bytes()) {
        Ok(n) => n,
        Err(err) => {
            warn!(err = %err, "lobby migration: failed to prune lobby messages");
            0
        }
    };
    let meta_rows_removed = match ms.delete_conversation_meta_row(lobby.as_bytes()) {
        Ok(n) => n,
        Err(err) => {
            warn!(
                err = %err,
                "lobby migration: failed to prune lobby conversation_meta row"
            );
            0
        }
    };
    (messages_removed, meta_rows_removed)
}

/// Run the BORU-DISC-18 startup migration: remove any stale saved lobby
/// conversation from the persisted room list and chat history.
///
/// Scans:
/// 1. The conversation store (SQLite `kv_store`, plus the legacy
///    `conversations.json` fallback) for entries whose topic equals
///    [`legacy_lobby_topic`], and removes them.
/// 2. The SQLite `messages` table (`message_store.db`) for messages on the
///    legacy lobby topic, and removes them (plus their `conversation_meta`
///    sidebar row).
///
/// Unrelated public rooms are untouched: the filter is exactly the canonical
/// lobby topic. The migration is idempotent — a second run removes nothing.
///
/// Returns a [`LobbyMigrationReport`] describing what was removed (all zeros
/// when there was nothing to do).
pub fn migrate_legacy_lobby(data_dir: &Path, storage: Option<&Storage>) -> LobbyMigrationReport {
    let conversations_removed = prune_persisted_conversations(data_dir, storage);
    let (messages_removed, meta_rows_removed) = prune_persisted_messages(data_dir);

    let report = LobbyMigrationReport {
        conversations_removed,
        messages_removed,
        meta_rows_removed,
    };
    if !report.is_empty() {
        info!(
            conversations_removed = report.conversations_removed,
            messages_removed = report.messages_removed,
            meta_rows_removed = report.meta_rows_removed,
            "lobby migration: removed stale saved lobby from persisted state"
        );
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversations::ConversationEntry;
    use crate::proto::TopicId;
    use crate::topic_derivation::public_room_topic;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        dir.push(format!("boru-lobby-migration-{name}-{suffix}"));
        dir
    }

    fn topic(byte: u8) -> TopicId {
        TopicId::from_bytes([byte; 32])
    }

    fn other_public_room_topic() -> TopicId {
        public_room_topic(0x00, "coffee-chat", 1)
    }

    // ── Derivation ─────────────────────────────────────────────────────

    #[test]
    fn legacy_lobby_topic_matches_canonical_derivation() {
        assert_eq!(
            legacy_lobby_topic(),
            public_room_topic(0x00, "public-lobby", 1),
            "legacy lobby topic must be the canonical mainnet public-lobby topic"
        );
    }

    #[test]
    fn is_legacy_lobby_topic_only_matches_canonical_topic() {
        let lobby = legacy_lobby_topic();
        assert!(is_legacy_lobby_topic(&lobby));

        // A user-created public room with a different name is NOT the lobby.
        assert!(!is_legacy_lobby_topic(&other_public_room_topic()));
        // A different network's lobby is NOT the legacy mainnet lobby.
        assert!(!is_legacy_lobby_topic(&public_room_topic(
            0x01,
            "public-lobby",
            1
        )));
        assert!(!is_legacy_lobby_topic(&public_room_topic(
            0x02,
            "public-lobby",
            1
        )));
        // A different version of the lobby is NOT the legacy lobby.
        assert!(!is_legacy_lobby_topic(&public_room_topic(
            0x00,
            "public-lobby",
            2
        )));
        // A direct-chat topic is NOT the lobby.
        assert!(!is_legacy_lobby_topic(&topic(0xAA)));
    }

    // ── In-memory pruning ──────────────────────────────────────────────

    #[test]
    fn prune_conversation_store_removes_lobby_keeps_public_room() {
        let dir = temp_dir("prune-conv");
        let mut store = ConversationStore::empty_at(&dir);

        let lobby = legacy_lobby_topic();
        let public = other_public_room_topic();
        store.upsert(ConversationEntry::new(lobby, "", "Public Lobby"));
        store.upsert(ConversationEntry::new(public, "", "Coffee Chat"));
        assert_eq!(store.len(), 2);

        let removed = prune_conversation_store(&mut store);

        assert_eq!(removed, 1, "exactly the lobby entry is removed");
        assert_eq!(store.len(), 1);
        assert!(store.find(&lobby).is_none(), "lobby entry removed");
        assert!(
            store.find(&public).is_some(),
            "unrelated public room untouched"
        );
    }

    #[test]
    fn prune_conversation_store_is_idempotent() {
        let dir = temp_dir("prune-conv-idem");
        let mut store = ConversationStore::empty_at(&dir);
        store.upsert(ConversationEntry::new(
            other_public_room_topic(),
            "",
            "Coffee Chat",
        ));

        assert_eq!(prune_conversation_store(&mut store), 0);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn prune_chat_history_removes_lobby_keeps_public_room() {
        let dir = temp_dir("prune-history");
        let mut store = ChatHistoryStore::empty_at(&dir);

        let lobby = legacy_lobby_topic();
        let public = other_public_room_topic();
        store.push(crate::chat_history::HistoryEntry::new(
            lobby,
            "sender",
            Vec::new(),
            "text",
            "lobby message",
        ));
        store.push(crate::chat_history::HistoryEntry::new(
            public,
            "sender",
            Vec::new(),
            "text",
            "public room message",
        ));

        let removed = prune_chat_history(&mut store);

        assert_eq!(removed, 1, "exactly the lobby entry is removed");
        assert_eq!(store.entries.len(), 1);
        assert_eq!(store.entries[0].topic, public, "public room history intact");
    }
}
