//! BORU-DISC-18 integration tests: the startup lobby migration removes a
//! persisted legacy auto-lobby conversation (room-list entry + per-topic chat
//! history) while leaving unrelated public rooms untouched.
//!
//! The fixture simulates an install upgraded from an old Boru version: the
//! data directory contains a `conversations.json` (or SQLite conversation
//! store) with both the canonical legacy lobby topic and a user-created
//! public room, plus a `message_store.db` with messages on both topics.
//! After `migrate_legacy_lobby` runs, the lobby is gone and the public room
//! survives.

use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use boru_core::{
    conversations::{ConversationEntry, ConversationStore},
    lobby_migration::{is_legacy_lobby_topic, legacy_lobby_topic, migrate_legacy_lobby},
    proto::TopicId,
    public_room::PublicNetwork,
    storage::Storage,
    store::MessageStore,
    topic_derivation::public_room_topic,
};

// ── Helpers ────────────────────────────────────────────────────────────────

fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("boru-lobby-migration-it-{name}-{suffix}"));
    dir
}

/// A user-created public room — a DIFFERENT topic from the legacy lobby, so
/// it must survive the migration untouched.
fn public_room_topic_other() -> TopicId {
    public_room_topic(0x00, "coffee-chat", 1)
}

fn msg_hash(byte: u8) -> [u8; 32] {
    [byte; 32]
}

fn sender_key(byte: u8) -> [u8; 32] {
    [byte ^ 0x5A; 32]
}

/// Insert one chat message into the SQLite `messages` table (which also
/// creates a `conversation_meta` sidebar row, like `import_legacy_history`).
fn insert_message(ms: &MessageStore, topic: &TopicId, hash_byte: u8, body: &str) {
    let inserted = ms
        .insert_chat_message(
            &msg_hash(hash_byte),
            topic.as_bytes(),
            &sender_key(hash_byte),
            1_000_000 + hash_byte as u64,
            "text",
            body,
            None,
            None,
            &sender_key(hash_byte),
        )
        .expect("insert chat message");
    assert!(inserted, "message must be inserted");
}

/// Build the fixture data directory: conversation store (JSON by default, or
/// SQLite when `store_in_sqlite`) containing a legacy lobby room and a normal
/// public room, plus a `message_store.db` with one message on each topic.
/// Returns the data directory and the two topics.
fn build_fixture(name: &str, store_in_sqlite: bool) -> (PathBuf, TopicId, TopicId) {
    let dir = temp_dir(name);
    std::fs::create_dir_all(&dir).expect("create fixture dir");

    let lobby = legacy_lobby_topic();
    let public = public_room_topic_other();
    assert_ne!(lobby, public, "fixture topics must differ");

    // Conversation store: legacy lobby room + normal public room.
    let mut conv_store = ConversationStore::empty_at(&dir);
    conv_store.upsert(ConversationEntry::new(lobby, "", "Public Lobby"));
    conv_store.upsert(ConversationEntry::new(public, "", "Coffee Chat"));
    if store_in_sqlite {
        let storage = Storage::open(&dir).expect("open storage");
        conv_store
            .save_to_sqlite(&storage)
            .expect("save conversation store to SQLite");
    } else {
        conv_store.save().expect("save conversation store to JSON");
    }

    // Chat history: one message on each topic.
    let ms = MessageStore::open(dir.join("message_store.db")).expect("open message store");
    insert_message(&ms, &lobby, 1, "hello lobby");
    insert_message(&ms, &public, 2, "hello public room");

    (dir, lobby, public)
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Fixture with a JSON conversation store + SQLite chat history: after the
/// migration the lobby room and its messages are gone; the public room and
/// its messages survive.
#[test]
fn migrate_legacy_lobby_removes_json_lobby_keeps_public_room() {
    let (dir, lobby, public) = build_fixture("json", false);
    let storage = Storage::open(&dir).expect("open storage");

    let report = migrate_legacy_lobby(&dir, Some(&storage));

    // The JSON conversation store had exactly one lobby entry.
    assert_eq!(report.conversations_removed, 1);
    // One lobby message + one lobby conversation_meta row were removed.
    assert_eq!(report.messages_removed, 1);
    assert_eq!(report.meta_rows_removed, 1);

    // Room list reloaded from disk: lobby gone, public room intact.
    let reloaded = ConversationStore::load(&dir).expect("reload conversation store");
    assert_eq!(reloaded.len(), 1, "only the public room remains");
    assert!(reloaded.find(&lobby).is_none(), "lobby room removed");
    assert!(
        reloaded.find(&public).is_some(),
        "public room survives migration"
    );

    // Chat history reloaded from SQLite: lobby messages gone, public intact.
    let ms = MessageStore::open(dir.join("message_store.db")).expect("reopen message store");
    assert_eq!(
        ms.count_messages_for_topic(lobby.as_bytes())
            .expect("count lobby"),
        0,
        "lobby messages removed"
    );
    assert_eq!(
        ms.count_messages_for_topic(public.as_bytes())
            .expect("count public"),
        1,
        "public room messages untouched"
    );
}

/// Fixture where the conversation store lives in the SQLite `kv_store`
/// (the primary path used by current builds): same outcome.
#[test]
fn migrate_legacy_lobby_removes_sqlite_lobby_keeps_public_room() {
    let (dir, lobby, public) = build_fixture("sqlite", true);
    let storage = Storage::open(&dir).expect("open storage");

    let report = migrate_legacy_lobby(&dir, Some(&storage));

    assert_eq!(report.conversations_removed, 1, "SQLite store lobby pruned");
    assert_eq!(report.messages_removed, 1);

    let reloaded = ConversationStore::load_from_sqlite(&storage, &dir);
    assert_eq!(reloaded.len(), 1);
    assert!(reloaded.find(&lobby).is_none(), "lobby room removed");
    assert!(
        reloaded.find(&public).is_some(),
        "public room survives migration"
    );
}

/// A clean install (no lobby anywhere) must be a no-op — the migration never
/// touches other topics and never creates files that were not there.
#[test]
fn migrate_legacy_lobby_is_noop_on_clean_install() {
    let dir = temp_dir("clean");
    std::fs::create_dir_all(&dir).expect("create clean dir");
    let storage = Storage::open(&dir).expect("open storage");

    let first = migrate_legacy_lobby(&dir, Some(&storage));
    assert!(first.is_empty(), "clean install removes nothing");

    let second = migrate_legacy_lobby(&dir, Some(&storage));
    assert!(second.is_empty(), "idempotent — second run removes nothing");
}

/// The migration never touches a public room whose topic is NOT the legacy
/// lobby, even when a dev/test-network lobby topic is present (they derive
/// from a different network byte and are unrelated rooms).
#[test]
fn migrate_legacy_lobby_only_matches_canonical_mainnet_topic() {
    let dev_lobby = public_room_topic(0x01, "public-lobby", 1);
    let test_lobby = public_room_topic(0x02, "public-lobby", 1);
    let other_version = public_room_topic(0x00, "public-lobby", 2);

    assert!(is_legacy_lobby_topic(&legacy_lobby_topic()));
    assert!(!is_legacy_lobby_topic(&dev_lobby));
    assert!(!is_legacy_lobby_topic(&test_lobby));
    assert!(!is_legacy_lobby_topic(&other_version));
    assert!(!is_legacy_lobby_topic(&public_room_topic_other()));
    // Sanity: the canonical mainnet lobby is what the tracker/advertisement
    // layer treats as the lobby, so the filter is aligned with the codebase.
    assert_eq!(
        legacy_lobby_topic(),
        boru_core::public_room::public_lobby_topic(PublicNetwork::Mainnet)
    );
}
