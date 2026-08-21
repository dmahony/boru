// ── Tests ─────────────────────────────────────────────────────────────

use super::*;
use crate::reactions::ReactionEvent;

#[test]
fn received_public_profile_roundtrips_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let peer = iroh::SecretKey::from_bytes(&[7; 32]).public();
    let profile = crate::user_profile::PublicUserProfile {
        display_name: "Alice".to_string(),
        bio: "Available".to_string(),
        avatar_identifier: Some("blob:avatar".to_string()),
        shared_files: Vec::new(),
    };
    {
        let storage = Storage::open(dir.path()).unwrap();
        storage
            .upsert_received_profile(&peer, &profile, 1_000)
            .unwrap();
    }
    let storage = Storage::open(dir.path()).unwrap();
    let rows = storage.load_received_profiles(1_001).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].peer, peer);
    assert_eq!(rows[0].profile, profile);
}

#[test]
fn malformed_received_public_profile_is_skipped_and_removed() {
    let storage = Storage::memory().unwrap();
    let peer = iroh::SecretKey::from_bytes(&[8; 32]).public();
    storage
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO received_peer_profiles
                 (peer_public_key, payload, received_at_ms, updated_at_ms)
                 VALUES (?1, ?2, 1, 1)",
                rusqlite::params![peer.as_bytes().as_slice(), [0xff_u8, 0x00_u8]],
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(())
        })
        .unwrap();
    assert!(storage.load_received_profiles(2).unwrap().is_empty());
    let remaining: i64 = storage
        .with_conn(|conn| {
            Ok(conn
                .query_row(
                    "SELECT COUNT(*) FROM received_peer_profiles",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?)
        })
        .unwrap();
    assert_eq!(remaining, 0);
}

#[test]
fn room_authorization_survives_restart_and_keeps_event_order() {
    let topic = crate::proto::TopicId::from_bytes([0x42; 32]);
    let owner = iroh::SecretKey::from_bytes(&[1; 32]);
    let member = iroh::SecretKey::from_bytes(&[2; 32]);
    let mut state = crate::authorization::AuthorizationState::new(topic, owner.public());
    state
        .admit_member(member.public(), crate::authorization::Role::Member)
        .unwrap();
    let event = crate::authorization::AuthorizationEvent::sign(
        &owner,
        topic,
        1,
        member.public(),
        crate::authorization::AuthorizationAction::Revoke {
            permission: crate::authorization::Permission::PinMessages,
        },
    )
    .unwrap();
    state.apply(&event).unwrap();

    let storage = Storage::memory().unwrap();
    storage.save_room_authorization(&topic, &state, &event).unwrap();
    let (restored, events) = storage.load_room_authorization(&topic).unwrap().unwrap();
    assert_eq!(restored, state);
    assert_eq!(events, vec![event]);
    assert!(!restored.allows(&member.public(), crate::authorization::Permission::PinMessages));
}

// ── Room-directory hide preferences (BORU-DIR-12, PDF Task 4.3) ──

/// The hide preference persists across restarts: set → read →
/// reopen → still present, and removing works both directions.
#[test]
fn room_hidden_ids_roundtrip_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let id_a = [0xAAu8; 32];
    let id_b = [0xBBu8; 32];

    {
        let storage = Storage::open(dir.path()).unwrap();
        assert!(storage.room_hidden_ids().unwrap().is_empty());

        storage.set_room_hidden(&id_a, true).unwrap();
        storage.set_room_hidden(&id_b, true).unwrap();
        let ids = storage.room_hidden_ids().unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&id_a));
        assert!(ids.contains(&id_b));

        // Idempotent hide: setting true twice keeps one entry.
        storage.set_room_hidden(&id_a, true).unwrap();
        assert_eq!(storage.room_hidden_ids().unwrap().len(), 2);

        // Un-hide one.
        storage.set_room_hidden(&id_a, false).unwrap();
        let ids = storage.room_hidden_ids().unwrap();
        assert_eq!(ids, vec![id_b]);
    }

    // Reopen: the preference survived the restart.
    let storage = Storage::open(dir.path()).unwrap();
    assert_eq!(storage.room_hidden_ids().unwrap(), vec![id_b]);

    // Un-hide the last one.
    storage.set_room_hidden(&id_b, false).unwrap();
    assert!(storage.room_hidden_ids().unwrap().is_empty());
}

/// Room ids are stored as exactly-32-byte identifiers; a malformed
/// payload is tolerated (bad entries skipped, valid ones returned).
#[test]
fn room_hidden_ids_skips_malformed_payloads() {
    let storage = Storage::memory().unwrap();
    // Corrupt the kv payload directly (bad hex / short id).
    storage
        .kv_set(Storage::ROOM_HIDDEN_IDS_KEY, "[\"zzzz\"]")
        .unwrap();
    assert!(storage.room_hidden_ids().unwrap().is_empty());

    let id = [0x11u8; 32];
    storage.set_room_hidden(&id, true).unwrap();
    assert_eq!(storage.room_hidden_ids().unwrap(), vec![id]);
}

// ── Documentation consistency ────────────────────────────────
//
// BORU-AUDIT-24: the architecture docs must reference the current
// SQLite schema version so maintainers and AI agents are not misled by
// a stale version number. When `CURRENT_SCHEMA_VERSION` is bumped, this
// test fails until the docs are updated in the same change.

fn read_repo_doc(rel: &str) -> String {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let path = Path::new(manifest).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read repo doc {rel} ({}): {e}; docs must be committed \
                 alongside the schema constant so this check can run",
            path.display()
        )
    })
}

#[test]
fn docs_reference_current_schema_version() {
    let n = CURRENT_SCHEMA_VERSION;

    // Canonical schema doc must pin the exact constant line.
    let storage_doc = read_repo_doc("docs/message-storage-design.md");
    assert!(
        storage_doc.contains(&format!("CURRENT_SCHEMA_VERSION: u32 = {n}")),
        "docs/message-storage-design.md does not state \
             `CURRENT_SCHEMA_VERSION: u32 = {n}` — update it when bumping the schema"
    );

    // Top-level architecture doc must name the schema version.
    let arch = read_repo_doc("docs/ARCHITECTURE.md");
    assert!(
        arch.contains(&format!("schema v{n}")) || arch.contains(&format!("V{n} schema")),
        "ARCHITECTURE.md does not mention schema v{n} — update the storage section"
    );
    assert!(
        arch.contains("CURRENT_SCHEMA_VERSION"),
        "ARCHITECTURE.md should point at CURRENT_SCHEMA_VERSION in src/storage.rs"
    );

    // Migration guide's before/after table must track the current version.
    let migration = read_repo_doc("docs/migration-guide.md");
    assert!(
        migration.contains(&format!("V{n}")),
        "docs/migration-guide.md schema-version row does not mention V{n}"
    );
}

// ── V1 message tables ──────────────────────────────────────────

fn random_public_key() -> iroh::PublicKey {
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = (i % 256) as u8;
    }
    iroh::PublicKey::from_bytes(&bytes).unwrap()
}

#[test]
fn v1_inbox_idempotent_insert() {
    let storage = Storage::memory().unwrap();
    let msg_id = [1u8; 32];
    let env = StoredEnvelope {
        msg_id,
        conversation_id: [2u8; 32],
        author_user_id: random_public_key(),
        author_device_id: random_public_key(),
        created_at_ms: 1000,
        expires_at_ms: 5000,
        ciphertext: bytes::Bytes::from(vec![1, 2, 3]),
        signature: [3u8; 64],
        acked_at_ms: None,
    };
    storage.insert_inbox(&env).unwrap();
    storage.insert_inbox(&env).unwrap(); // idempotent
    let fetched = storage.get_inbox(&msg_id).unwrap().unwrap();
    assert_eq!(fetched.msg_id, env.msg_id);
}

#[test]
fn v1_outbox_flow() {
    let storage = Storage::memory().unwrap();
    let msg_id = [1u8; 32];
    let recipient = random_public_key();
    storage.enqueue_outbox(&msg_id, recipient, 1000).unwrap();
    let due = storage.fetch_due_outbox(500).unwrap();
    assert!(due.is_empty());
    let due = storage.fetch_due_outbox(1500).unwrap();
    assert_eq!(due.len(), 1);
    storage
        .record_attempt(&msg_id, recipient, 3000, Some("timeout"))
        .unwrap();
    let due = storage.fetch_due_outbox(1500).unwrap();
    assert!(due.is_empty());
    storage.mark_acked(&msg_id, recipient).unwrap();
}

#[test]
fn v1_contacts_crud() {
    let storage = Storage::memory().unwrap();
    let user = random_public_key();
    let device = random_public_key();
    storage
        .upsert_contact(&user, &device, None, b"key-data", 1000, 5000)
        .unwrap();
    let contacts = storage.list_contacts().unwrap();
    assert_eq!(contacts.len(), 1);
}

#[test]
fn v1_sync_cursor_crud() {
    let storage = Storage::memory().unwrap();
    let peer = random_public_key();
    storage
        .upsert_sync_cursor(&peer, Some(b"clock-data"), 2000)
        .unwrap();
    let cursors = storage.list_sync_cursors().unwrap();
    assert_eq!(cursors.len(), 1);
}

#[test]
fn v1_get_sync_cursor_by_peer() {
    let storage = Storage::memory().unwrap();
    let peer = iroh::SecretKey::generate().public();
    let other = iroh::SecretKey::generate().public();

    // Returns None for unregistered peer.
    assert!(storage.get_sync_cursor(&peer).unwrap().is_none());

    // Upsert and verify per-peer lookup.
    storage
        .upsert_sync_cursor(&peer, Some(b"clock-1"), 1000)
        .unwrap();
    let cursor = storage.get_sync_cursor(&peer).unwrap().unwrap();
    assert_eq!(cursor.last_sync_at_ms, 1000);
    assert_eq!(cursor.last_seen_msg_clock, Some(b"clock-1".to_vec()));

    // Other peer still returns None.
    assert!(storage.get_sync_cursor(&other).unwrap().is_none());

    // Update and verify.
    storage
        .upsert_sync_cursor(&peer, Some(b"clock-2"), 2000)
        .unwrap();
    let cursor = storage.get_sync_cursor(&peer).unwrap().unwrap();
    assert_eq!(cursor.last_sync_at_ms, 2000);
    assert_eq!(cursor.last_seen_msg_clock, Some(b"clock-2".to_vec()));
}

#[test]
fn v1_query_pending_outbound_for_recipient_pagination() {
    let storage = Storage::memory().unwrap();
    let sender_sk = iroh::SecretKey::generate();
    let sender = sender_sk.public();
    let recipient_sk = iroh::SecretKey::generate();
    let recipient_id = recipient_sk.public();
    let recipient = MailboxPublicKey {
        identity: recipient_id,
        encryption: [0u8; 32],
    };
    let conv_id = [1u8; 32];

    // Insert 5 outbound messages from sender to recipient.
    for i in 0..5u64 {
        let request_key = format!("req-{i}");
        let plaintext = format!("hello {i}");
        storage
            .queue_outgoing_dm(
                conv_id,
                sender,
                &request_key,
                &plaintext,
                recipient,
                &sender_sk,
            )
            .unwrap();
    }

    // Query with max_count=3 should return 3 with has_more=true.
    let (page, has_more) = storage
        .query_pending_outbound_for_recipient(&recipient_id, 0, 3, 10_000_000)
        .unwrap();
    assert_eq!(page.len(), 3);
    assert!(has_more, "should have more pages");

    // Query with max_count=10 should return all 5 with has_more=false.
    let (page, has_more) = storage
        .query_pending_outbound_for_recipient(&recipient_id, 0, 10, 10_000_000)
        .unwrap();
    assert_eq!(page.len(), 5);
    assert!(!has_more, "should not have more pages");

    // Query scoped by recipient: other_recipient gets nothing.
    let other_recipient = iroh::SecretKey::generate().public();
    let (page, has_more) = storage
        .query_pending_outbound_for_recipient(&other_recipient, 0, 10, 10_000_000)
        .unwrap();
    assert_eq!(page.len(), 0);
    assert!(!has_more);

    // Query with since_ms beyond current time.
    let far_future_ms = now_ms() + 86_400_000; // 1 day in the future
    let (page, _) = storage
        .query_pending_outbound_for_recipient(&recipient_id, far_future_ms, 10, 10_000_000)
        .unwrap();
    assert_eq!(page.len(), 0);
}

#[test]
fn v1_sync_dedup_replay_protection() {
    let storage = Storage::memory().unwrap();
    let sender_sk = iroh::SecretKey::generate();
    let sender = sender_sk.public();
    let recipient_sk = iroh::SecretKey::generate();
    let recipient_id = recipient_sk.public();
    let recipient = MailboxPublicKey {
        identity: recipient_id,
        encryption: [0u8; 32],
    };
    let conv_id = [2u8; 32];

    // Insert 3 outbound messages, capturing their raw message_id.
    let mut msg_ids: Vec<[u8; 32]> = Vec::new();
    for i in 0..3u64 {
        let request_key = format!("dedup-req-{i}");
        let plaintext = format!("dedup-test {i}");
        let outgoing = storage
            .queue_outgoing_dm(
                conv_id,
                sender,
                &request_key,
                &plaintext,
                recipient,
                &sender_sk,
            )
            .unwrap();
        msg_ids.push(outgoing.message_id);
    }

    // First call returns all 3.
    let (page, _) = storage
        .query_pending_outbound_for_recipient(&recipient_id, 0, 10, 10_000_000)
        .unwrap();
    assert_eq!(page.len(), 3, "first call should return all 3");

    // Record first 2 message IDs as already served.
    storage
        .record_sync_served(&recipient_id, &msg_ids[..2])
        .unwrap();

    // Second call should return only the 3rd envelope.
    let (page2, _) = storage
        .query_pending_outbound_for_recipient(&recipient_id, 0, 10, 10_000_000)
        .unwrap();
    assert_eq!(page2.len(), 1, "second call should skip served envelopes");
    let second_mid = page2[0].message_id();
    let third_mid = page[2].message_id();
    assert_eq!(
        second_mid, third_mid,
        "remaining envelope should be the 3rd"
    );

    // Record the 3rd as served too.
    storage
        .record_sync_served(&recipient_id, &msg_ids[2..])
        .unwrap();

    // Third call should return nothing.
    let (page3, _) = storage
        .query_pending_outbound_for_recipient(&recipient_id, 0, 10, 10_000_000)
        .unwrap();
    assert_eq!(
        page3.len(),
        0,
        "third call should return nothing when all served"
    );

    // Stale dedup pruning should not affect recent entries.
    storage.prune_sync_dedup().unwrap();
    let (page4, _) = storage
        .query_pending_outbound_for_recipient(&recipient_id, 0, 10, 10_000_000)
        .unwrap();
    assert_eq!(
        page4.len(),
        0,
        "after prune, still nothing (entries are fresh)"
    );

    // Other recipient still sees nothing.
    let other_recipient = iroh::SecretKey::generate().public();
    let (page5, _) = storage
        .query_pending_outbound_for_recipient(&other_recipient, 0, 10, 10_000_000)
        .unwrap();
    assert_eq!(page5.len(), 0);
}

// ── V2 file-object tables ──────────────────────────────────────

#[test]
fn v2_file_object_round_trip() {
    let storage = Storage::memory().unwrap();
    storage
        .put_file_object("abc123", 500, "text/plain", "readme.txt", b"hello world")
        .unwrap();
    assert!(storage.file_object_exists("abc123").unwrap());
    let obj = storage.get_file_object("abc123").unwrap().unwrap();
    assert_eq!(obj.size, 500);
    assert_eq!(obj.filename, "readme.txt");
    assert_eq!(obj.data.as_deref(), Some(&b"hello world"[..]));
}

#[test]
fn chat_upload_is_owned_by_profile_and_available_in_catalogue() {
    let storage = Storage::memory().unwrap();
    let data = b"chat-upload-data";
    let hash = storage
        .register_chat_upload("alice", "photo.webp", "image/webp", data)
        .unwrap();

    let alice_files = storage.list_shared_files("alice", true).unwrap();
    assert_eq!(alice_files.len(), 1);
    assert_eq!(alice_files[0].content_hash, hash);
    assert_eq!(alice_files[0].display_filename, "photo.webp");
    assert!(storage.list_shared_files("bob", true).unwrap().is_empty());
    assert_eq!(
        storage
            .get_file_object(&hash)
            .unwrap()
            .unwrap()
            .data
            .as_deref(),
        Some(&data[..])
    );
    assert!(storage.get_manifest_state("alice").unwrap().is_some());
}

#[test]
fn v2_message_attachments() {
    let storage = Storage::memory().unwrap();
    storage
        .put_file_object("hash1", 100, "image/png", "photo.png", b"binary")
        .unwrap();
    let att_id = storage
        .attach_file_to_message(42, "hash1", "photo.png", 0)
        .unwrap();
    assert!(att_id > 0);
    let attachments = storage.get_message_attachments(42).unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].content_hash, "hash1");

    // Find messages referencing a file.
    let msg_ids = storage.find_messages_for_file("hash1").unwrap();
    assert_eq!(msg_ids, vec![42]);

    // Remove.
    assert!(storage.remove_message_attachment(att_id).unwrap());
    assert!(storage.get_message_attachments(42).unwrap().is_empty());
}

#[test]
fn delete_chat_history_removes_target_attachments_only() {
    let storage = Storage::memory().unwrap();
    storage
        .put_file_object("target-file", 10, "text/plain", "target.txt", b"target")
        .unwrap();
    storage
        .put_file_object("other-file", 10, "text/plain", "other.txt", b"other")
        .unwrap();
    storage
        .attach_file_to_message(10, "target-file", "target.txt", 0)
        .unwrap();
    storage
        .attach_file_to_message(20, "other-file", "other.txt", 0)
        .unwrap();

    let topic = [7u8; 32];
    assert_eq!(storage.delete_chat_history(&topic, &[10]).unwrap(), 1);
    assert!(storage.get_message_attachments(10).unwrap().is_empty());
    assert_eq!(storage.get_message_attachments(20).unwrap().len(), 1);
    // Content-addressed objects remain available to unrelated ownership.
    assert!(storage.file_object_exists("target-file").unwrap());
    assert!(storage.file_object_exists("other-file").unwrap());
    assert_eq!(storage.delete_chat_history(&topic, &[10]).unwrap(), 0);
}

#[test]
fn v2_shared_files_and_collections() {
    let storage = Storage::memory().unwrap();
    storage
        .put_file_object("hash2", 200, "application/pdf", "doc.pdf", b"pdf-data")
        .unwrap();
    storage
        .upsert_shared_file(
            "hash2",
            "alice_key",
            "meta-1",
            "doc.pdf",
            Some("My document"),
            true,
        )
        .unwrap();
    let files = storage.list_shared_files("alice_key", true).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].display_filename, "doc.pdf");

    // Collections.
    let coll_id = storage
        .ensure_collection("alice_key", "docs", Some("My docs"))
        .unwrap();
    storage.add_to_collection(coll_id, "hash2", 0).unwrap();
    let items = storage.list_collection_items(coll_id).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].content_hash, "hash2");
}

#[test]
fn v2_catalogue_limits_reject_new_entries_but_allow_updates() {
    let limits = CatalogueLimitsConfig {
        max_files_per_catalogue: 1,
        max_collections: 1,
        max_entries_per_collection: 1,
        ..Default::default()
    };
    let storage = Storage::memory_with_catalogue_limits(limits).unwrap();
    storage
        .put_file_object("hash-a", 100, "text/plain", "a.txt", b"a")
        .unwrap();
    storage
        .put_file_object("hash-b", 200, "text/plain", "b.txt", b"b")
        .unwrap();

    storage
        .upsert_shared_file("hash-a", "alice_key", "meta-a", "a.txt", None, true)
        .unwrap();
    // Updating an existing offered file must succeed even when the catalogue is full.
    storage
        .upsert_shared_file(
            "hash-a",
            "alice_key",
            "meta-a-2",
            "a-renamed.txt",
            None,
            true,
        )
        .unwrap();
    let files = storage.list_shared_files("alice_key", true).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].metadata_id, "meta-a-2");
    assert_eq!(files[0].display_filename, "a-renamed.txt");

    let err = storage
        .upsert_shared_file("hash-b", "alice_key", "meta-b", "b.txt", None, true)
        .unwrap_err();
    let err_msg = err.to_string();
    assert!(err_msg.contains("files"));
    assert!(storage
        .get_shared_file("alice_key", "hash-b")
        .unwrap()
        .is_none());

    let coll_id = storage
        .ensure_collection("alice_key", "docs", Some("docs"))
        .unwrap();
    // Re-using the existing collection is allowed at the limit.
    let same_coll_id = storage
        .ensure_collection("alice_key", "docs", Some("docs v2"))
        .unwrap();
    assert_eq!(same_coll_id, coll_id);
    let err = storage
        .ensure_collection("alice_key", "photos", Some("photos"))
        .unwrap_err();
    let err_msg = err.to_string();
    assert!(err_msg.contains("collections"));
    assert_eq!(storage.list_collections("alice_key").unwrap().len(), 1);

    storage.add_to_collection(coll_id, "hash-a", 0).unwrap();
    storage.add_to_collection(coll_id, "hash-a", 1).unwrap();
    let items = storage.list_collection_items(coll_id).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].position, 1);
    let err = storage.add_to_collection(coll_id, "hash-b", 2).unwrap_err();
    let err_msg = err.to_string();
    assert!(err_msg.contains("entries"));
    assert_eq!(storage.list_collection_items(coll_id).unwrap().len(), 1);
}

#[test]
fn v2_permissions() {
    let storage = Storage::memory().unwrap();
    storage
        .put_file_object("hash3", 50, "text/plain", "note.txt", b"note")
        .unwrap();
    storage
        .grant_permission("hash3", "alice", "bob", "read", None)
        .unwrap();
    assert!(storage.check_permission("hash3", "bob", "read").unwrap());
    assert!(!storage
        .check_permission("hash3", "bob", "download")
        .unwrap());
    assert!(storage
        .revoke_permission("hash3", "alice", "bob", "read")
        .unwrap());
    assert!(!storage.check_permission("hash3", "bob", "read").unwrap());
}

#[test]
fn v2_downloads_state_machine() {
    let storage = Storage::memory().unwrap();
    // Insert a file_object first to satisfy the FK constraint.
    storage
        .put_file_object("hash4", 1024, "application/octet-stream", "large.bin", b"")
        .unwrap();
    let id = storage.create_download("hash4", "bob_peer", 1024).unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "queued");

    storage.update_download_progress(id, 512, "active").unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.bytes_downloaded, 512);

    storage
        .fail_download(id, "connection reset", Some(5000))
        .unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "failed");
    assert_eq!(dl.last_error.as_deref(), Some("connection reset"));
    assert_eq!(dl.retry_count, 1);
}

#[test]
fn pause_preserves_partial_state_and_is_idempotent() {
    let storage = Storage::memory().unwrap();
    storage
        .put_file_object(
            "pause-hash",
            4096,
            "application/octet-stream",
            "payload.bin",
            b"",
        )
        .unwrap();
    // Exercise pause from each active phase used by the transfer worker.
    for (index, state) in ["resolving_peer", "requesting_permission", "downloading"]
        .into_iter()
        .enumerate()
    {
        let hash = format!("pause-hash-{index}");
        storage
            .put_file_object(&hash, 4096, "application/octet-stream", "payload.bin", b"")
            .unwrap();
        let id = storage.create_download(&hash, "peer-a", 4096).unwrap();
        storage.update_download_progress(id, 1234, state).unwrap();
        storage.pause_download(id).unwrap();
        let paused = storage.get_download(id).unwrap().unwrap();
        assert_eq!(paused.state, "paused");
        assert_eq!(paused.content_hash, hash);
        assert_eq!(paused.remote_peer, "peer-a");
        assert_eq!(paused.bytes_downloaded, 1234);
        assert_eq!(paused.total_bytes, 4096);

        // A repeated user action must not reset progress or fail.
        storage.pause_download(id).unwrap();
        let repeated = storage.get_download(id).unwrap().unwrap();
        assert_eq!(repeated.state, "paused");
        assert_eq!(repeated.bytes_downloaded, 1234);

        // Move back to an active state before exercising the next phase.
        // Pausing is intentionally idempotent, but it must not implicitly
        // resume work just because a test (or caller) wants another pause.
        storage.resume_download(id).unwrap();
    }
}

#[test]
fn paused_download_rejects_late_worker_progress() {
    let storage = Storage::memory().unwrap();
    storage
        .put_file_object("pause-race", 100, "application/octet-stream", "x.bin", b"")
        .unwrap();
    let id = storage
        .create_download("pause-race", "peer-b", 100)
        .unwrap();
    storage
        .update_download_progress(id, 40, "transferring")
        .unwrap();
    storage.pause_download(id).unwrap();

    let result = storage.update_download_progress(id, 80, "transferring");
    assert!(
        result.is_err(),
        "stale active work must be rejected after pause"
    );
    let paused = storage.get_download(id).unwrap().unwrap();
    assert_eq!(paused.state, "paused");
    assert_eq!(paused.bytes_downloaded, 40);
}

#[test]
fn pause_rejects_terminal_and_unknown_downloads() {
    let storage = Storage::memory().unwrap();
    storage
        .put_file_object("pause-terminal", 1, "application/octet-stream", "x", b"")
        .unwrap();
    let id = storage
        .create_download("pause-terminal", "peer-c", 1)
        .unwrap();
    storage.update_download_progress(id, 1, "complete").unwrap();
    assert!(storage.pause_download(id).is_err());
    assert!(storage.pause_download(i64::MAX).is_err());
}

#[test]
fn resume_revalidates_descriptor_before_transfer() {
    let storage = Storage::memory().unwrap();
    storage
        .put_file_object(
            "resume-hash",
            4096,
            "application/octet-stream",
            "payload.bin",
            b"",
        )
        .unwrap();
    let id = storage
        .create_download("resume-hash", "peer-a", 4096)
        .unwrap();
    storage
        .update_download_progress(id, 1024, "downloading")
        .unwrap();
    storage.pause_download(id).unwrap();
    storage.resume_download(id).unwrap();

    let resolving = storage.get_download(id).unwrap().unwrap();
    assert_eq!(resolving.state, "resolving_peer");
    assert_eq!(resolving.bytes_downloaded, 1024);
    assert_eq!(resolving.content_hash, "resume-hash");

    let mismatch = storage.accept_resumed_descriptor(id, "changed-hash", 4096);
    assert!(mismatch.is_err());
    let stopped = storage.get_download(id).unwrap().unwrap();
    assert_eq!(stopped.state, "version_mismatch");
    assert!(stopped.last_error.unwrap().contains("hash mismatch"));
}

#[test]
fn resume_accepts_fresh_descriptor_and_is_idempotent_while_active() {
    let storage = Storage::memory().unwrap();
    storage
        .put_file_object(
            "resume-ok",
            200,
            "application/octet-stream",
            "payload.bin",
            b"",
        )
        .unwrap();
    let id = storage.create_download("resume-ok", "peer-b", 200).unwrap();
    storage.pause_download(id).unwrap();
    storage.resume_download(id).unwrap();
    storage
        .accept_resumed_descriptor(id, "resume-ok", 200)
        .unwrap();
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        "downloading"
    );
    storage.resume_download(id).unwrap();
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        "downloading"
    );
}

#[test]
fn expired_resume_descriptor_keeps_download_paused() {
    let storage = Storage::memory().unwrap();
    storage
        .put_file_object("resume-expired", 20, "application/octet-stream", "x", b"")
        .unwrap();
    let id = storage
        .create_download("resume-expired", "peer-d", 20)
        .unwrap();
    storage.pause_download(id).unwrap();
    storage.resume_download(id).unwrap();
    let result = storage.accept_resumed_descriptor_at(id, "resume-expired", 20, 99, 100);
    assert!(result.is_err());
    let download = storage.get_download(id).unwrap().unwrap();
    assert_eq!(download.state, "paused");
    assert!(download.last_error.unwrap().contains("expired"));
}

#[test]
fn resume_rejects_terminal_and_unknown_downloads() {
    let storage = Storage::memory().unwrap();
    assert!(storage.resume_download(i64::MAX).is_err());
    storage
        .put_file_object("resume-terminal", 1, "application/octet-stream", "x", b"")
        .unwrap();
    let id = storage
        .create_download("resume-terminal", "peer-c", 1)
        .unwrap();
    storage.update_download_progress(id, 1, "complete").unwrap();
    assert!(storage.resume_download(id).is_err());
}

#[test]
fn record_local_file_object_fast_path_round_trip() {
    let storage = Storage::memory().unwrap();
    let hash = "a".repeat(64);
    let path = "/home/user/videos/clip.mp4";

    // Unknown source path → no record.
    assert!(storage
        .file_object_hash_by_source_path(path)
        .unwrap()
        .is_none());

    // Record a chat-sent local file (no inline data; blob_hash set).
    storage
        .record_local_file_object(
            &hash,
            1024,
            "application/octet-stream",
            "clip.mp4",
            path,
            &hash,
        )
        .unwrap();

    // Lookup by source path returns the content hash.
    assert_eq!(
        storage.file_object_hash_by_source_path(path).unwrap(),
        Some(hash.clone())
    );

    // The row is a blob-reference file object: no inline data, and the
    // source path is recorded so the file-access handler can serve it.
    let object = storage.get_file_object(&hash).unwrap().unwrap();
    assert_eq!(object.data, None);
    assert_eq!(object.source_path.as_deref(), Some(path));

    // Re-recording the same content under a new source path updates the
    // mapping (idempotent upsert, no duplicate rows).
    let path2 = "/home/user/videos/clip-copy.mp4";
    storage
        .record_local_file_object(
            &hash,
            1024,
            "application/octet-stream",
            "clip.mp4",
            path2,
            &hash,
        )
        .unwrap();
    assert_eq!(
        storage.file_object_hash_by_source_path(path2).unwrap(),
        Some(hash.clone())
    );
    assert!(storage
        .file_object_hash_by_source_path(path)
        .unwrap()
        .is_none());
}

#[test]
fn v2_profile_manifest_revision_increments() {
    let storage = Storage::memory().unwrap();
    let rev1 = storage
        .bump_manifest_revision("alice_key", "hash-a")
        .unwrap();
    assert_eq!(rev1, 1);
    let rev2 = storage
        .bump_manifest_revision("alice_key", "hash-b")
        .unwrap();
    assert_eq!(rev2, 2);
    let state = storage.get_manifest_state("alice_key").unwrap().unwrap();
    assert_eq!(state.revision, 2);
    assert_eq!(state.manifest_hash, "hash-b");
}

#[test]
fn schema_version_is_recorded() {
    let storage = Storage::memory().unwrap();
    let version: u32 = storage
        .with_conn(|conn| {
            Ok(conn
                .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                    row.get(0)
                })
                .map_err(|e| anyhow!("{}", e))?)
        })
        .unwrap();
    assert_eq!(version, CURRENT_SCHEMA_VERSION);
}

#[test]
fn imported_file_object() {
    let storage = Storage::memory().unwrap();
    storage
        .put_imported_file_object(
            "abc789",
            9999,
            "video/mp4",
            "movie.mp4",
            "blob-xyz-hash",
            "peer123",
        )
        .unwrap();
    let obj = storage.get_file_object("abc789").unwrap().unwrap();
    assert_eq!(obj.size, 9999);
    assert!(obj.data.is_none()); // imported files have no inline data
}

#[test]
fn foreign_key_enforcement_prevents_orphan_attachment() {
    let storage = Storage::memory().unwrap();
    // Attaching to a non-existent file_object should fail.
    let result = storage.attach_file_to_message(1, "no-such-hash", "x.txt", 0);
    assert!(result.is_err());
}

// ── Crash and corruption resilience tests (Step 16) ────────────────

#[test]
fn test_unsupported_schema_version_returns_error() {
    // If the DB has a schema_version higher than CURRENT_SCHEMA_VERSION,
    // opening should return a clear error.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("boru.db");

    // Create a valid DB first
    {
        let storage = Storage::open(dir.path()).unwrap();
        // Verify current schema version
        let version: u32 = storage
            .with_conn(|conn| {
                Ok(conn
                    .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                        row.get(0)
                    })
                    .map_err(|e| anyhow!("{e}"))?)
            })
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    // Manually insert a higher version to simulate a future-schema DB
    {
        let conn = Connection::open(&db_path).unwrap();
        let future_version = CURRENT_SCHEMA_VERSION + 1;
        conn.execute(
            "INSERT INTO schema_version (version, applied_at_ms) VALUES (?1, ?2)",
            params![future_version, 9999999999i64],
        )
        .unwrap();
    }

    // Reopening should fail
    let result = Storage::open(dir.path());
    assert!(result.is_err());
    let err = format!("{}", result.unwrap_err());
    assert!(
        err.contains("schema version") || err.contains("newer version"),
        "expected version-mismatch error, got: {err}"
    );
}

#[test]
fn test_integrity_check_fails_on_corrupt_db_storage() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("boru.db");

    // Create a valid DB
    {
        let _storage = Storage::open(dir.path()).unwrap();
    }

    // Corrupt it
    std::fs::write(&db_path, b"garbage data").unwrap();

    // Opening should fail
    let result = Storage::open(dir.path());
    assert!(result.is_err());
}

#[test]
fn test_crash_left_sent_outbox_recovered_storage() {
    let dir = tempfile::tempdir().unwrap();
    let msg_id = [1u8; 32];
    let recipient = random_public_key();

    // First session: insert with Sent state
    {
        let storage = Storage::open(dir.path()).unwrap();
        let env = StoredEnvelope {
            msg_id,
            conversation_id: [2u8; 32],
            author_user_id: random_public_key(),
            author_device_id: random_public_key(),
            created_at_ms: 1000,
            expires_at_ms: 5000,
            ciphertext: bytes::Bytes::from(vec![1, 2, 3]),
            signature: [3u8; 64],
            acked_at_ms: None,
        };
        storage.insert_inbox(&env).unwrap();
        storage.enqueue_outbox(&msg_id, recipient, 1000).unwrap();
        storage
            .record_attempt(&msg_id, recipient, 2000, Some("in_flight"))
            .unwrap();
    }

    // Second session: crash recovery
    {
        let storage = Storage::open(dir.path()).unwrap();
        let due = storage.fetch_due_outbox(now_ms() + 1000).unwrap();
        let row = due.iter().find(|r| r.msg_id == msg_id);
        assert!(
            row.is_some(),
            "crash-left Sent outbox should be recovered to Pending"
        );
    }
}

#[test]
fn test_stale_pending_timestamp_reset_storage() {
    let dir = tempfile::tempdir().unwrap();
    let msg_id = [1u8; 32];
    let recipient = random_public_key();
    let far_future = now_ms() + 86_400_000;

    // First session: insert pending row with future timestamp via raw SQL
    {
        let storage = Storage::open(dir.path()).unwrap();
        let conn = storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO outbox (msg_id, recipient_device_id, status, attempts, next_attempt_at_ms)
                 VALUES (?1, ?2, ?3, 0, ?4)",
            params![
                msg_id.as_slice(),
                recipient.as_bytes(),
                crate::store::DeliveryStatus::Pending as u8,
                far_future as i64,
            ],
        )
        .unwrap();
    }

    // Second session: timestamp should be reset
    {
        let storage = Storage::open(dir.path()).unwrap();
        let due = storage.fetch_due_outbox(now_ms() + 1000).unwrap();
        let row = due.iter().find(|r| r.msg_id == msg_id);
        assert!(
            row.is_some(),
            "stale pending timestamp should be recovered to due"
        );
    }
}

#[test]
fn test_partial_migration_resumes_on_reopen() {
    // Verify that a partially-applied migration can resume on reopen.
    // The IF NOT EXISTS in each migration makes re-runs idempotent.
    let dir = tempfile::tempdir().unwrap();

    // Open and close — runs all migrations
    {
        let _s = Storage::open(dir.path()).unwrap();
    }

    // Insert a fake partial state: remove v2–v7 tables, keep v1.
    {
        let db_path = dir.path().join("boru.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
                 DROP TABLE IF EXISTS file_verification;
                 DROP TABLE IF EXISTS file_replacements;
                 DROP TABLE IF EXISTS shared_file_permissions;
                 DROP TABLE IF EXISTS file_collection_items;
                 DROP TABLE IF EXISTS file_collections;
                 DROP TABLE IF EXISTS message_attachments;
                 DROP TABLE IF EXISTS shared_files;
                 DROP TABLE IF EXISTS downloads;
                 DROP TABLE IF EXISTS profile_manifest_state;
                 DROP TABLE IF EXISTS file_objects;
                 DROP TABLE IF EXISTS dm_outbox;
                 DROP TABLE IF EXISTS dm_messages;
                 DROP TABLE IF EXISTS dm_sender_sequences;
                 DROP TABLE IF EXISTS dm_conversations;
                 DROP TABLE IF EXISTS dm_acknowledgements;
                 DROP TABLE IF EXISTS sync_dedup;
                 DROP TABLE IF EXISTS outbox;
                 DELETE FROM schema_version;",
        )
        .unwrap();
    }

    // Reopen — should re-apply v2 migration
    {
        let storage = Storage::open(dir.path()).unwrap();
        // Verify v2 tables exist again
        assert!(storage.file_object_exists("test").is_ok());
        // Schema version should be back to current
        let version: u32 = storage
            .with_conn(|conn| {
                Ok(conn
                    .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                        row.get(0)
                    })
                    .map_err(|e| anyhow!("{e}"))?)
            })
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }
}

#[test]
fn outbox_claim_is_exclusive_and_releases_on_failure() {
    let storage = Storage::memory().unwrap();
    let msg_id = [7u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();
    let claimed = storage
        .claim_due_outbox(100, "worker-a", 1_000, 1)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.lease_owner.as_deref(), Some("worker-a"));
    assert!(storage
        .claim_due_outbox(100, "worker-b", 1_000, 1)
        .unwrap()
        .is_none());
    assert!(storage
        .finish_outbox_attempt(&msg_id, peer, "worker-a", false, 100, Some("reset"))
        .unwrap());
    assert!(storage
        .claim_due_outbox(100, "worker-b", 1_000, 1)
        .unwrap()
        .is_some());
}

#[test]
fn outbox_stale_lease_is_reclaimable() {
    let storage = Storage::memory().unwrap();
    let msg_id = [8u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();
    storage.claim_due_outbox(100, "dead-worker", 10, 1).unwrap();
    assert!(storage
        .claim_due_outbox(109, "new-worker", 10, 1)
        .unwrap()
        .is_none());
    assert_eq!(storage.recover_stale_outbox_leases(110).unwrap(), 1);
    assert!(storage
        .claim_due_outbox(110, "new-worker", 10, 1)
        .unwrap()
        .is_some());
}

#[test]
fn outbox_lease_can_be_extended_only_by_owner() {
    let storage = Storage::memory().unwrap();
    let msg_id = [10u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();
    storage.claim_due_outbox(100, "worker-a", 10, 1).unwrap();
    assert!(!storage
        .extend_outbox_lease(&msg_id, peer, "worker-b", 100, 200)
        .unwrap());
    assert!(storage
        .extend_outbox_lease(&msg_id, peer, "worker-a", 100, 300)
        .unwrap());
    assert!(storage
        .claim_due_outbox(299, "worker-b", 10, 1)
        .unwrap()
        .is_none());
    assert!(storage
        .claim_due_outbox(300, "worker-b", 10, 1)
        .unwrap()
        .is_some());
}

#[test]
fn ack_clears_outbox_lease() {
    let storage = Storage::memory().unwrap();
    let msg_id = [11u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();
    storage.claim_due_outbox(100, "worker-a", 1_000, 1).unwrap();
    storage.mark_acked(&msg_id, peer).unwrap();
    let row = storage.fetch_due_outbox(100).unwrap();
    assert!(row.is_empty());
    let claimed = storage.claim_due_outbox(100, "worker-b", 10, 1).unwrap();
    assert!(claimed.is_none());
}

#[test]
fn outbox_claim_survives_restart_with_lease_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let msg_id = [9u8; 32];
    let peer = random_public_key();
    // Use real-time-based timestamps with a long lease so that
    // recover_crash_state (called by Storage::open) does not clear
    // the lease before we can test that it survives restart.
    let t0 = now_ms();
    {
        let storage = Storage::open(dir.path()).unwrap();
        storage.enqueue_outbox(&msg_id, peer, t0).unwrap();
        storage
            .claim_due_outbox(t0, "crashed-worker", 30_000, 1)
            .unwrap();
        // locked_until_ms = t0 + 30_000
    }
    // After reopen, recover_crash_state sees locked_until_ms
    // is still in the future — does NOT clear it.
    let storage = Storage::open(dir.path()).unwrap();
    // Lease still valid: claim with a different owner should fail.
    assert!(storage
        .claim_due_outbox(t0 + 100, "replacement", 10, 1)
        .unwrap()
        .is_none());
    // After lease expires, claim should succeed.
    assert!(storage
        .claim_due_outbox(t0 + 30_001, "replacement", 10, 1)
        .unwrap()
        .is_some());
}

#[test]
fn outbox_claim_query_is_bounded() {
    let storage = Storage::memory().unwrap();
    let peer = random_public_key();
    for id in 0..(MAX_OUTBOX_CLAIM_LIMIT + 5) {
        storage.enqueue_outbox(&[id as u8; 32], peer, 0).unwrap();
    }
    assert!(storage.fetch_due_outbox(0).unwrap().len() <= MAX_OUTBOX_CLAIM_LIMIT as usize);
}

// ── Comprehensive outbox claim/lease tests ──────────────────────────

/// Single worker claims an entry, completes delivery successfully.
/// Verifies: status → Sent, lease cleared, attempts incremented,
/// last_attempt_at_ms set, next_attempt_at_ms set for future retry.
#[test]
fn test_outbox_single_worker_successful_delivery() {
    let storage = Storage::memory().unwrap();
    let msg_id = [20u8; 32];
    let peer = random_public_key();

    // Enqueue at t=100 with next_attempt=100 (immediately due)
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // Claim at t=100
    let claimed = storage
        .claim_due_outbox(100, "worker-1", 30_000, 1)
        .unwrap()
        .expect("should claim the due entry");
    assert_eq!(claimed.msg_id, msg_id);
    assert_eq!(claimed.recipient_device_id, peer);
    assert_eq!(claimed.status, DeliveryStatus::Pending);
    assert_eq!(claimed.attempts, 0);
    assert_eq!(claimed.lease_owner.as_deref(), Some("worker-1"));
    assert!(claimed.locked_until_ms.is_some());
    assert_eq!(claimed.locked_until_ms.unwrap(), 100 + 30_000);

    // Finish with success — schedule next attempt far in the future
    let done = storage
        .finish_outbox_attempt(
            &msg_id, peer, "worker-1", true,    // success
            200_000, // next_attempt_at_ms (far future)
            None,    // no error
        )
        .unwrap();
    assert!(done, "finish_outbox_attempt should succeed");

    // Verify: the entry should NOT appear in fetch_due_outbox at t=100
    // because next_attempt_at_ms=200_000 is in the future.
    let due = storage.fetch_due_outbox(100).unwrap();
    assert!(
        due.iter().find(|r| r.msg_id == msg_id).is_none(),
        "successfully sent entry should not be due at t=100"
    );

    // But at t=200_000 it should become due again (for retry if needed)
    let due2 = storage.fetch_due_outbox(200_000).unwrap();
    let row = due2
        .iter()
        .find(|r| r.msg_id == msg_id)
        .expect("entry should be due at t=200000");
    assert_eq!(row.status, DeliveryStatus::Sent);
    assert_eq!(row.attempts, 1);
    assert!(row.last_attempt_at_ms.is_some());
    assert_eq!(row.last_error_code, None);
    assert_eq!(row.lease_owner, None);
    assert_eq!(row.locked_until_ms, None);
}

/// Two workers race for the same entry. Only one wins.
/// Verifies the losing worker's claim returns None and the winner's
/// lease fields are correctly set.
#[test]
fn test_outbox_two_competitors_one_wins() {
    let storage = Storage::memory().unwrap();
    let msg_id = [21u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // Worker A claims
    let claimed_a = storage
        .claim_due_outbox(100, "worker-a", 30_000, 1)
        .unwrap()
        .expect("worker-a should claim");
    assert_eq!(claimed_a.lease_owner.as_deref(), Some("worker-a"));
    assert_eq!(claimed_a.locked_until_ms, Some(30_100));

    // Worker B tries to claim — must fail because the row is locked
    let claimed_b = storage
        .claim_due_outbox(100, "worker-b", 30_000, 1)
        .unwrap();
    assert!(claimed_b.is_none(), "worker-b must not claim locked row");

    // Worker A releases
    assert!(storage
        .release_outbox_lease(&msg_id, peer, "worker-a")
        .unwrap());

    // Now worker B can claim
    let claimed_b2 = storage
        .claim_due_outbox(100, "worker-b", 30_000, 1)
        .unwrap()
        .expect("worker-b should claim after release");
    assert_eq!(claimed_b2.lease_owner.as_deref(), Some("worker-b"));
}

/// Multiple stale leases are recovered in a single batch.
#[test]
fn test_outbox_recover_stale_leases_batch() {
    let storage = Storage::memory().unwrap();
    let peer = random_public_key();

    for i in 0..3 {
        let id = [30 + i as u8; 32];
        storage.enqueue_outbox(&id, peer, 100).unwrap();
    }

    // "dead-worker" claims all 3 with a short lease (10ms)
    for i in 0..3 {
        let _id = [30 + i as u8; 32];
        storage
            .claim_due_outbox(100, "dead-worker", 10, 10)
            .unwrap();
    }

    // At t=109 lease still valid — nothing to recover
    assert_eq!(storage.recover_stale_outbox_leases(109).unwrap(), 0);

    // At t=110 all leases expired — recover all 3
    assert_eq!(storage.recover_stale_outbox_leases(110).unwrap(), 3);

    // Now a new worker can claim all 3
    for i in 0..3 {
        let claimed = storage
            .claim_due_outbox(110, "new-worker", 10, 10)
            .unwrap()
            .unwrap_or_else(|| panic!("entry {} should be claimable", i));
        assert_eq!(claimed.lease_owner.as_deref(), Some("new-worker"));
        // Release so the next loop iteration can claim too
        storage
            .release_outbox_lease(&[30 + i as u8; 32], peer, "new-worker")
            .unwrap();
    }
}

/// After the lease expires (locked_until_ms ≤ now_ms), a new worker
/// can claim the entry *without* explicitly calling recover_stale.
#[test]
fn test_outbox_lease_expiry_claimable_without_recovery() {
    let storage = Storage::memory().unwrap();
    let msg_id = [22u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // Worker A claims with short lease (10ms)
    storage.claim_due_outbox(100, "worker-a", 10, 1).unwrap();

    // At t=109: still locked — worker B cannot claim
    assert!(storage
        .claim_due_outbox(109, "worker-b", 10, 1)
        .unwrap()
        .is_none());

    // At t=110: lease expired — worker B CAN claim (claim_due_outbox
    // checks locked_until_ms <= now_ms in its WHERE clause)
    let claimed = storage
        .claim_due_outbox(110, "worker-b", 10, 1)
        .unwrap()
        .expect("worker-b should claim expired lease at t=110");
    assert_eq!(claimed.lease_owner.as_deref(), Some("worker-b"));
}

/// After release_outbox_lease, the entry is immediately claimable
/// at the same timestamp (no time advance needed).
#[test]
fn test_outbox_release_makes_immediately_claimable() {
    let storage = Storage::memory().unwrap();
    let msg_id = [23u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // Worker A claims
    storage
        .claim_due_outbox(100, "worker-a", 30_000, 1)
        .unwrap();

    // Worker A gracefully releases at t=100
    assert!(storage
        .release_outbox_lease(&msg_id, peer, "worker-a")
        .unwrap());

    // Worker B claims immediately at same t=100
    let claimed = storage
        .claim_due_outbox(100, "worker-b", 30_000, 1)
        .unwrap()
        .expect("worker-b should claim immediately after release");
    assert_eq!(claimed.lease_owner.as_deref(), Some("worker-b"));
}

/// Simulate a crash: a worker claims with a short lease, the process
/// restarts, and recover_crash_state clears the expired lease so the
/// entry becomes claimable.
#[test]
fn test_outbox_restart_clears_expired_lease() {
    let dir = tempfile::tempdir().unwrap();
    let msg_id = [24u8; 32];
    let peer = random_public_key();

    // First session: enqueue and claim with short lease (1ms)
    {
        let storage = Storage::open(dir.path()).unwrap();
        storage.enqueue_outbox(&msg_id, peer, 100).unwrap();
        storage.claim_due_outbox(100, "crash-worker", 1, 1).unwrap();
        // locked_until_ms = 100 + 1 = 101
    }
    // "crash" — process dies

    // Second session: recover_crash_state runs during Storage::open.
    // If the lease was set with locked_until_ms=101, and now real time
    // is much later, the lease should be cleared.
    {
        let storage = Storage::open(dir.path()).unwrap();
        // After recover_crash_state, the expired lease should be gone.
        // The entry should be claimable immediately.
        let claimed = storage
            .claim_due_outbox(
                now_ms(), // use real current time
                "recovery-worker",
                30_000,
                1,
            )
            .unwrap();
        assert!(
            claimed.is_some(),
            "expired lease should be cleared by recover_crash_state, making entry claimable"
        );
    }
}

/// finish_outbox_attempt must fail when called by a non-owner.
#[test]
fn test_outbox_finish_wrong_owner_rejected() {
    let storage = Storage::memory().unwrap();
    let msg_id = [25u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // Worker A claims
    storage
        .claim_due_outbox(100, "worker-a", 30_000, 1)
        .unwrap();

    // Worker B (non-owner) tries to finish — must fail
    let wrong = storage
        .finish_outbox_attempt(
            &msg_id, peer, "worker-b", // wrong owner
            true, 200_000, None,
        )
        .unwrap();
    assert!(!wrong, "non-owner must not finish the attempt");

    // Worker A (owner) finishes successfully
    let ok = storage
        .finish_outbox_attempt(&msg_id, peer, "worker-a", true, 200_000, None)
        .unwrap();
    assert!(ok, "owner must be able to finish the attempt");
}

/// release_outbox_lease must fail when called by a non-owner.
#[test]
fn test_outbox_release_wrong_owner_rejected() {
    let storage = Storage::memory().unwrap();
    let msg_id = [26u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // Worker A claims
    storage
        .claim_due_outbox(100, "worker-a", 30_000, 1)
        .unwrap();

    // Worker B tries to release — must fail
    assert!(!storage
        .release_outbox_lease(&msg_id, peer, "worker-b")
        .unwrap());

    // Worker A releases — must succeed
    assert!(storage
        .release_outbox_lease(&msg_id, peer, "worker-a")
        .unwrap());
}

/// Each call to finish_outbox_attempt increments the attempts counter.
#[test]
fn test_outbox_attempts_counter_increments() {
    let storage = Storage::memory().unwrap();
    let msg_id = [27u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // First attempt: attempts 0 → 1
    let claimed = storage
        .claim_due_outbox(100, "worker", 30_000, 1)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.attempts, 0);
    storage
        .finish_outbox_attempt(
            &msg_id,
            peer,
            "worker",
            false, // failure
            200,   // retry at t=200
            Some("err1"),
        )
        .unwrap();

    // Second attempt: attempts 1 → 2 (re-claim after release)
    let claimed2 = storage
        .claim_due_outbox(200, "worker", 30_000, 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed2.attempts, 1,
        "attempts should be 1 after first finish"
    );
    assert_eq!(claimed2.last_error_code.as_deref(), Some("err1"));
    storage
        .finish_outbox_attempt(&msg_id, peer, "worker", false, 300, Some("err2"))
        .unwrap();

    // Third attempt: attempts 2 → 3
    let claimed3 = storage
        .claim_due_outbox(300, "worker", 30_000, 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed3.attempts, 2,
        "attempts should be 2 after second finish"
    );
    assert_eq!(claimed3.last_error_code.as_deref(), Some("err2"));
}

/// fetch_due_outbox must not return entries that hold a live lease.
#[test]
fn test_outbox_fetch_due_excludes_live_leased_entries() {
    let storage = Storage::memory().unwrap();
    let msg_id = [28u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // Before claim: fetch_due_outbox returns the entry
    let before = storage.fetch_due_outbox(100).unwrap();
    assert!(before.iter().any(|r| r.msg_id == msg_id));

    // Claim with long lease
    storage.claim_due_outbox(100, "worker", 30_000, 1).unwrap();

    // After claim: fetch_due_outbox should NOT return it (live lease)
    let after = storage.fetch_due_outbox(100).unwrap();
    assert!(
        !after.iter().any(|r| r.msg_id == msg_id),
        "live-leased entry must not appear in fetch_due_outbox"
    );

    // After lease expires: fetch_due_outbox returns it again
    let expired = storage.fetch_due_outbox(130_001).unwrap();
    assert!(
        expired.iter().any(|r| r.msg_id == msg_id),
        "expired-lease entry must appear in fetch_due_outbox"
    );
}

/// Multiple entries are claimed in FIFO order by next_attempt_at_ms.
#[test]
fn test_outbox_multiple_entries_fifo_claim_order() {
    let storage = Storage::memory().unwrap();
    let peer = random_public_key();

    // Enqueue 3 entries with staggered next_attempt timestamps
    storage
        .enqueue_outbox(&[40u8; 32], peer, 300) // latest
        .unwrap();
    storage
        .enqueue_outbox(&[41u8; 32], peer, 200) // middle
        .unwrap();
    storage
        .enqueue_outbox(&[42u8; 32], peer, 100) // earliest
        .unwrap();

    // Claim should return earliest first
    let r1 = storage
        .claim_due_outbox(500, "worker", 30_000, 10)
        .unwrap()
        .expect("first claim");
    assert_eq!(
        r1.msg_id, [42u8; 32],
        "earliest next_attempt (100) should be first"
    );
    // Finish to push next_attempt far into the future so this entry
    // won't be picked up again by subsequent claims.
    storage
        .finish_outbox_attempt(&[42u8; 32], peer, "worker", false, 999_999, None)
        .unwrap();

    let r2 = storage
        .claim_due_outbox(500, "worker", 30_000, 10)
        .unwrap()
        .expect("second claim");
    assert_eq!(
        r2.msg_id, [41u8; 32],
        "middle next_attempt (200) should be second"
    );
    storage
        .finish_outbox_attempt(&[41u8; 32], peer, "worker", false, 999_999, None)
        .unwrap();

    let r3 = storage
        .claim_due_outbox(500, "worker", 30_000, 10)
        .unwrap()
        .expect("third claim");
    assert_eq!(
        r3.msg_id, [40u8; 32],
        "latest next_attempt (300) should be third"
    );
}

// ── Progress batch tests ──────────────────────────────────────────

#[test]
fn flush_progress_batch_writes_multiple_downloads() {
    let storage = Storage::memory().unwrap();
    let hash = "hash-a";
    storage
        .put_file_object(hash, 4096, "app/bin", "a.bin", b"data")
        .unwrap();
    let id1 = storage.create_download(hash, "peer1", 4096).unwrap();
    let id2 = storage.create_download(hash, "peer2", 4096).unwrap();
    let id3 = storage.create_download(hash, "peer3", 4096).unwrap();

    // Write all three in a single batch.
    let batch = [
        (id1, 1024u64, "downloading"),
        (id2, 2048u64, "downloading"),
        (id3, 3072u64, "downloading"),
    ];
    storage.flush_progress_batch(&batch).unwrap();

    let d1 = storage.get_download(id1).unwrap().unwrap();
    let d2 = storage.get_download(id2).unwrap().unwrap();
    let d3 = storage.get_download(id3).unwrap().unwrap();
    assert_eq!(d1.bytes_downloaded, 1024);
    assert_eq!(d1.state, "downloading");
    assert_eq!(d2.bytes_downloaded, 2048);
    assert_eq!(d2.state, "downloading");
    assert_eq!(d3.bytes_downloaded, 3072);
    assert_eq!(d3.state, "downloading");
}

#[test]
fn flush_progress_batch_skips_paused_downloads() {
    let storage = Storage::memory().unwrap();
    let hash = "hash-b";
    storage
        .put_file_object(hash, 100, "app/bin", "b.bin", b"test")
        .unwrap();
    let id1 = storage.create_download(hash, "peer1", 100).unwrap();
    let id2 = storage.create_download(hash, "peer2", 100).unwrap();

    // Pause id2.
    storage.pause_download(id2).unwrap();

    // Batch that includes the paused download — should not fail.
    let batch = [
        (id1, 50u64, "downloading"),
        (id2, 60u64, "downloading"), // paused — written to 0 rows
    ];
    storage.flush_progress_batch(&batch).unwrap();

    let d1 = storage.get_download(id1).unwrap().unwrap();
    let d2 = storage.get_download(id2).unwrap().unwrap();
    assert_eq!(d1.bytes_downloaded, 50, "active download updated");
    assert_eq!(d2.bytes_downloaded, 0, "paused download not modified");
}

#[test]
fn flush_progress_batch_empty_is_noop() {
    let storage = Storage::memory().unwrap();
    storage.flush_progress_batch(&[]).unwrap();
    // No panic — that's the test.
}

// ── Remote catalogue cache lifecycle ───────────────────────────

/// Helper: build a test catalogue signed with `sk` containing the given
/// file hashes, each as a [`RemoteSharedFile`] with a sensible default
/// display name.
fn make_test_catalogue(sk: &SecretKey, revision: u64, file_hashes: &[&str]) -> SignedFileCatalogue {
    let files: Vec<RemoteSharedFile> = file_hashes
        .iter()
        .enumerate()
        .map(|(i, h)| {
            RemoteSharedFile::new(
                *h,
                format!("file-{i}.data"),
                None,
                1024,
                "application/octet-stream",
                None,
                1,
            )
        })
        .collect();
    SignedFileCatalogue::sign(sk, revision, now_ms(), vec![], files)
}

/// Assert that the remote catalogue meta for `peer` matches the
/// expected revision.
fn assert_cached_revision(storage: &Storage, peer: &PublicKey, expected_rev: u64) {
    let meta = storage
        .get_remote_catalogue_meta(peer)
        .expect("get_remote_catalogue_meta");
    assert!(
        meta.is_some(),
        "expected cached meta for {peer} at rev {expected_rev}"
    );
    assert_eq!(
        meta.unwrap().revision,
        expected_rev,
        "cached revision for {peer}"
    );
}

/// Assert that the set of content hashes cached for `peer` matches
/// `expected` (order-insensitive).
fn assert_cached_files(storage: &Storage, peer: &PublicKey, expected: &[&str]) {
    let rows = storage
        .get_remote_shared_files(peer)
        .expect("get_remote_shared_files");
    let mut actual: Vec<&str> = rows.iter().map(|r| r.content_hash.as_str()).collect();
    actual.sort();
    let mut exp: Vec<&str> = expected.to_vec();
    exp.sort();
    assert_eq!(actual, exp, "cached file hashes for {peer}");
}

/// Assert that NO remote catalogue meta exists for `peer`.
#[allow(dead_code)]
fn assert_no_cached_meta(storage: &Storage, peer: &PublicKey) {
    let meta = storage
        .get_remote_catalogue_meta(peer)
        .expect("get_remote_catalogue_meta");
    assert!(meta.is_none(), "expected no cached meta for {peer}");
}

#[test]
fn first_fetch_stores_profile_and_revision() {
    let storage = Storage::memory().unwrap();
    let sk = SecretKey::generate();
    let pk = sk.public();

    let cat = make_test_catalogue(&sk, 1, &["hash_a", "hash_b"]);
    storage
        .replace_remote_catalogue(&cat)
        .expect("replace_remote_catalogue");

    // Meta — revision and peer are recorded.
    assert_cached_revision(&storage, &pk, 1);

    // Files — both hashes are present.
    assert_cached_files(&storage, &pk, &["hash_a", "hash_b"]);

    // The peer key in meta matches the catalogue owner.
    let meta = storage.get_remote_catalogue_meta(&pk).unwrap().unwrap();
    assert_eq!(
        meta.peer,
        pk.to_string(),
        "meta peer matches catalogue owner"
    );
}

#[test]
fn not_modified_reuses_cached_content_without_replacing() {
    let storage = Storage::memory().unwrap();
    let sk = SecretKey::generate();
    let pk = sk.public();

    // 1) First fetch stores file A.
    let cat1 = make_test_catalogue(&sk, 1, &["hash_a"]);
    storage
        .replace_remote_catalogue(&cat1)
        .expect("first replace");

    assert_cached_revision(&storage, &pk, 1);
    assert_cached_files(&storage, &pk, &["hash_a"]);

    // 2) NotModified — we do NOT call replace_remote_catalogue because
    //    the server responded with NotModified.  The cached data should
    //    still be revision 1 and file A only.
    //    (No second replace_remote_catalogue call here.)

    assert_cached_revision(&storage, &pk, 1);
    assert_cached_files(&storage, &pk, &["hash_a"]);

    // 3) Even if we *were* to call replace_remote_catalogue with the
    //    identical catalogue, the content should remain unchanged.
    storage
        .replace_remote_catalogue(&cat1)
        .expect("re-replace identical");

    assert_cached_revision(&storage, &pk, 1);
    assert_cached_files(&storage, &pk, &["hash_a"]);
}

#[test]
fn newer_revision_replaces_cached_content() {
    let storage = Storage::memory().unwrap();
    let sk = SecretKey::generate();
    let pk = sk.public();

    // 1) First catalogue has file A, file B at rev 1.
    let cat1 = make_test_catalogue(&sk, 1, &["hash_a", "hash_b"]);
    storage
        .replace_remote_catalogue(&cat1)
        .expect("first replace");
    assert_cached_revision(&storage, &pk, 1);
    assert_cached_files(&storage, &pk, &["hash_a", "hash_b"]);

    // 2) Newer catalogue replaces file B with file C at rev 2.
    let cat2 = make_test_catalogue(&sk, 2, &["hash_a", "hash_c"]);
    storage
        .replace_remote_catalogue(&cat2)
        .expect("second replace");

    // Revision bumped.
    assert_cached_revision(&storage, &pk, 2);

    // hash_b is gone, hash_c is new, hash_a persists.
    assert_cached_files(&storage, &pk, &["hash_a", "hash_c"]);
    // Explicitly: hash_b must NOT be in the cached set.
    let rows = storage
        .get_remote_shared_files(&pk)
        .expect("get_remote_shared_files");
    for row in &rows {
        assert_ne!(
            row.content_hash, "hash_b",
            "stale file hash_b should have been removed"
        );
    }
}

#[test]
fn file_removal_reflected_after_refresh() {
    let storage = Storage::memory().unwrap();
    let sk = SecretKey::generate();
    let pk = sk.public();

    // 1) Catalogue with three files.
    let cat1 = make_test_catalogue(&sk, 5, &["keep_a", "keep_b", "remove_me"]);
    storage
        .replace_remote_catalogue(&cat1)
        .expect("first replace");
    assert_cached_files(&storage, &pk, &["keep_a", "keep_b", "remove_me"]);

    // 2) Server removes "remove_me" and bumps revision.
    let cat2 = make_test_catalogue(&sk, 6, &["keep_a", "keep_b"]);
    storage
        .replace_remote_catalogue(&cat2)
        .expect("second replace");

    assert_cached_revision(&storage, &pk, 6);
    assert_cached_files(&storage, &pk, &["keep_a", "keep_b"]);
    // Explicitly assert the removed file is gone.
    let rows = storage
        .get_remote_shared_files(&pk)
        .expect("get_remote_shared_files");
    for row in &rows {
        assert_ne!(
            row.content_hash, "remove_me",
            "removed file should not appear in cached files"
        );
    }
}

// ── Durable delivery claiming (Phase 6) ─────────────────────────

/// Claim a pending outbox row via `claim_pending_deliveries`:
/// status transitions from Pending → Sending, `last_attempt_at_ms` is
/// set, and the returned row has the `Sending` status.
#[test]
fn test_claim_pending_deliveries_transitions_to_sending() {
    let storage = Storage::memory().unwrap();
    let msg_id = [40u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    let claimed = storage.claim_pending_deliveries(5, 100).unwrap();
    assert_eq!(claimed.len(), 1, "should claim 1 row");
    let row = &claimed[0];
    assert_eq!(row.msg_id, msg_id);
    assert_eq!(row.status, DeliveryStatus::Sending);
    assert_eq!(row.last_attempt_at_ms, Some(100));
    assert_eq!(row.attempts, 0);
}

/// Two workers calling `claim_pending_deliveries` on the same row:
/// only the first succeeds, the second gets nothing.
#[test]
fn test_claim_pending_deliveries_two_competitors_one_wins() {
    let storage = Storage::memory().unwrap();
    let msg_id = [41u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // Worker A claims
    let claimed_a = storage.claim_pending_deliveries(5, 100).unwrap();
    assert_eq!(claimed_a.len(), 1);
    assert_eq!(claimed_a[0].status, DeliveryStatus::Sending);

    // Worker B tries to claim — row is now Sending, should get nothing
    let claimed_b = storage.claim_pending_deliveries(5, 100).unwrap();
    assert!(
        claimed_b.is_empty(),
        "worker B should not claim a Sending row"
    );
}

/// Rows in `Sent` status (from a prior failed attempt) are also
/// eligible for claiming when `next_attempt_at_ms` has arrived.
#[test]
fn test_claim_pending_deliveries_includes_sent_rows() {
    let storage = Storage::memory().unwrap();
    let msg_id = [42u8; 32];
    let peer = random_public_key();

    // Insert a row directly in Sent status with due next_attempt
    let conn = storage.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO outbox (msg_id, recipient_device_id, status, attempts, next_attempt_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            msg_id.as_slice(),
            peer.as_bytes(),
            DeliveryStatus::Sent as u8,
            1,   // one prior attempt
            100, // due now
        ],
    )
    .unwrap();
    drop(conn);

    let claimed = storage.claim_pending_deliveries(5, 100).unwrap();
    assert_eq!(claimed.len(), 1, "should claim the Sent row");
    assert_eq!(claimed[0].status, DeliveryStatus::Sending);
    assert_eq!(claimed[0].attempts, 1, "retry count preserved");
}

/// Rows scheduled in the future are not claimable.
#[test]
fn test_claim_pending_deliveries_respects_future_timestamps() {
    let storage = Storage::memory().unwrap();
    let msg_id = [43u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 200).unwrap(); // due at 200

    let claimed = storage.claim_pending_deliveries(5, 100).unwrap();
    assert!(claimed.is_empty(), "row is not due until 200");
}

/// A delivered-and-acked row is terminal — no claiming occurs.
#[test]
fn test_claim_pending_deliveries_excludes_acked_rows() {
    let storage = Storage::memory().unwrap();
    let msg_id = [44u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();
    storage.mark_acked(&msg_id, peer).unwrap();

    let claimed = storage.claim_pending_deliveries(5, 200).unwrap();
    assert!(claimed.is_empty(), "acked rows should not be claimable");
}

/// Recover stale Sending rows — those stuck in Sending with an old
/// `last_attempt_at_ms` are moved back to Pending.
#[test]
fn test_recover_stale_sending_deliveries() {
    let storage = Storage::memory().unwrap();
    let msg_id = [45u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // Manually put the row in Sending with a stale last_attempt_at_ms
    let conn = storage.conn.lock().unwrap();
    conn.execute(
        "UPDATE outbox SET status = ?1, last_attempt_at_ms = ?2
             WHERE msg_id = ?3",
        params![
            DeliveryStatus::Sending as u8,
            50i64, // last_attempt_at_ms = 50 (stale, cutoff = now - 60s)
            msg_id.as_slice(),
        ],
    )
    .unwrap();
    drop(conn);

    // Recover at t=70000 (stale cutoff = 10000, so 50 < 10000 = stale)
    let recovered = storage.recover_stale_sending_deliveries(70000).unwrap();
    assert_eq!(recovered, 1, "should recover 1 stale row");

    // Now the row should be back in Pending and claimable
    let claimed = storage.claim_pending_deliveries(5, 70000).unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].status, DeliveryStatus::Sending);
}

/// Fresh Sending rows (claimed recently) are NOT recovered.
#[test]
fn test_recover_stale_sending_does_not_touch_recent_rows() {
    let storage = Storage::memory().unwrap();
    let msg_id = [46u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // Claim the row (puts it in Sending with last_attempt_at_ms = 1000)
    let claimed = storage.claim_pending_deliveries(5, 1000).unwrap();
    assert_eq!(claimed.len(), 1);

    // Recover at t=1020 — stale cutoff = 1020 - 60000 = -58980
    // last_attempt_at_ms=1000 > -58980, so NOT stale
    let recovered = storage.recover_stale_sending_deliveries(1020).unwrap();
    assert_eq!(recovered, 0, "recently claimed row should not be recovered");
}

/// `claim_pending_deliveries` respects the lease guard: a row locked by
/// another worker (via the lease mechanism) is not claimable.
#[test]
fn test_claim_pending_deliveries_respects_locks() {
    let storage = Storage::memory().unwrap();
    let msg_id = [47u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // Leased by another worker (as if claimed via claim_due_outbox)
    storage
        .claim_due_outbox(100, "lease-worker", 30_000, 1)
        .unwrap();

    // claim_pending_deliveries should skip the leased row
    let claimed = storage.claim_pending_deliveries(5, 100).unwrap();
    assert!(claimed.is_empty(), "leased row should not be claimable");

    // After lease expires, it becomes claimable
    let claimed2 = storage.claim_pending_deliveries(5, 100 + 30_001).unwrap();
    assert_eq!(claimed2.len(), 1);
    assert_eq!(claimed2[0].status, DeliveryStatus::Sending);
}

// ── mark_sent / mark_acked idempotency (Phase 7) ─────────────────

/// `mark_sent` transitions Sending → Sent.  Calling it twice is
/// harmless: the second call is a no-op that doesn't error.
#[test]
fn test_mark_sent_is_idempotent() {
    let storage = Storage::memory().unwrap();
    let msg_id = [50u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // Claim puts row in Sending
    let _claimed = storage.claim_pending_deliveries(5, 100).unwrap();
    assert_eq!(_claimed.len(), 1);
    assert_eq!(_claimed[0].status, DeliveryStatus::Sending);

    // First mark_sent: Sending → Sent (next_attempt_at_ms = 100, so
    // the row is still immediately claimable for retry).
    storage.mark_sent(&msg_id, peer, 100).unwrap();

    // Sent rows are still claimable
    let claimed_after = storage.claim_pending_deliveries(5, 100).unwrap();
    assert!(
        !claimed_after.is_empty(),
        "Sent row should still be claimable"
    );

    // Second mark_sent: no-op, stays Sent
    storage.mark_sent(&msg_id, peer, 100).unwrap();
}

/// `mark_sent` on an already-acked row is a no-op (guarded by WHERE).
#[test]
fn test_mark_sent_noop_on_acked() {
    let storage = Storage::memory().unwrap();
    let msg_id = [51u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // Complete the lifecycle without marking sent
    storage.mark_acked(&msg_id, peer).unwrap();

    // mark_sent on Acked row should not change status
    storage.mark_sent(&msg_id, peer, 200).unwrap();

    // Acked rows are not claimable
    let claimed = storage.claim_pending_deliveries(5, 200).unwrap();
    assert!(claimed.is_empty(), "acked rows should not be claimable");
}

/// `mark_acked` transitions to Acked.  Calling it twice is idempotent.
#[test]
fn test_mark_acked_is_idempotent() {
    let storage = Storage::memory().unwrap();
    let msg_id = [52u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // First mark_acked — transitions to Acked
    storage.mark_acked(&msg_id, peer).unwrap();

    // Verify: Acked rows are not claimable
    let claimed = storage.claim_pending_deliveries(5, 200).unwrap();
    assert!(claimed.is_empty(), "acked rows should not be claimable");

    // Second mark_acked — idempotent
    storage.mark_acked(&msg_id, peer).unwrap();

    // Still not claimable
    let claimed2 = storage.claim_pending_deliveries(5, 200).unwrap();
    assert!(claimed2.is_empty(), "still acked after second call");
}

/// Integration: Sending → mark_sent → mark_acked is the expected
/// lifecycle: transport sends bytes (Sent), then protocol ACK arrives
/// (Acked).
#[test]
fn test_lifecycle_sending_sent_acked() {
    let storage = Storage::memory().unwrap();
    let msg_id = [53u8; 32];
    let peer = random_public_key();
    storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

    // Claim → Sending
    let claimed = storage.claim_pending_deliveries(5, 100).unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].status, DeliveryStatus::Sending);

    // transport delivers bytes → Sent (next_attempt_at_ms = 100, so
    // the row remains claimable for retry)
    storage.mark_sent(&msg_id, peer, 100).unwrap();

    // Sent rows are claimable
    let claimed_after = storage.claim_pending_deliveries(5, 100).unwrap();
    assert!(!claimed_after.is_empty(), "Sent row should be claimable");

    // protocol ACK arrives → Acked
    storage.mark_acked(&msg_id, peer).unwrap();

    // Acked rows are not claimable
    let claimed_after_ack = storage.claim_pending_deliveries(5, 200).unwrap();
    assert!(claimed_after_ack.is_empty(), "acked row not claimable");

    // Can't go backwards: mark_sent after acked does nothing
    storage.mark_sent(&msg_id, peer, 200).unwrap();

    // Still not claimable
    let claimed_final = storage.claim_pending_deliveries(5, 200).unwrap();
    assert!(claimed_final.is_empty(), "still acked after mark_sent");
}

#[test]
fn group_storage_crud_and_idempotency() {
    let storage = Storage::memory().unwrap();
    let group = GroupRow {
        group_id: [1; 32],
        name: "Team".into(),
        description: "Desc".into(),
        owner_public_key: vec![9; 32],
        current_epoch: 0,
        created_at_ms: 10,
        updated_at_ms: 10,
        archived: false,
    };
    storage.create_group(&group).unwrap();
    storage.create_group(&group).unwrap();
    assert_eq!(storage.list_groups(false).unwrap(), vec![group.clone()]);
    assert!(storage
        .update_group_metadata(&group.group_id, "Renamed", "New", 20, false)
        .unwrap());
    assert_eq!(
        storage.get_group(&group.group_id).unwrap().unwrap().name,
        "Renamed"
    );
    let member = GroupMemberRow {
        group_id: group.group_id,
        public_key: vec![2; 32],
        role: "Member".into(),
        joined_at_ms: 11,
        invited_by: Some(vec![9; 32]),
        epoch_joined: 1,
        state: "Invited".into(),
    };
    storage.add_group_member(&member).unwrap();
    storage.add_group_member(&member).unwrap();
    assert_eq!(
        storage.list_group_members(&group.group_id).unwrap().len(),
        1
    );
    assert!(storage
        .update_group_member(&group.group_id, &member.public_key, "Member", "Active")
        .unwrap());
    assert_eq!(
        storage.list_group_members(&group.group_id).unwrap()[0].state,
        "Active"
    );
}

#[test]
fn group_epochs_and_pending_invites_survive_crud() {
    let storage = Storage::memory().unwrap();
    let group = GroupRow {
        group_id: [4; 32],
        name: "G".into(),
        description: "".into(),
        owner_public_key: vec![1; 32],
        current_epoch: 0,
        created_at_ms: 1,
        updated_at_ms: 1,
        archived: false,
    };
    storage.create_group(&group).unwrap();
    let epoch = GroupEpochRow {
        group_id: group.group_id,
        epoch: 1,
        topic_id: [5; 32].into(),
        discovery_secret: vec![6; 32],
        created_at_ms: 2,
    };
    storage.create_group_epoch(&epoch).unwrap();
    storage.create_group_epoch(&epoch).unwrap();
    assert_eq!(
        storage.get_current_group_epoch(&group.group_id).unwrap(),
        Some(epoch)
    );
    let invite = GroupInviteRow {
        invite_id: [7; 32],
        group_id: group.group_id,
        inviter_public_key: vec![1; 32],
        recipient_public_key: vec![2; 32],
        epoch: 1,
        status: "Pending".into(),
        created_at_ms: 3,
        expires_at_ms: 100,
        ticket: String::new(),
        group_name: "Test Group".to_string(),
    };
    storage.create_group_invite(&invite).unwrap();
    storage.create_group_invite(&invite).unwrap();
    assert_eq!(
        storage.get_pending_group_invites(&[2; 32], 50).unwrap(),
        vec![invite.clone()]
    );
    assert!(storage
        .update_group_invite_state(&invite.invite_id, "Accepted")
        .unwrap());
    assert!(storage
        .get_pending_group_invites(&[2; 32], 50)
        .unwrap()
        .is_empty());
}

#[test]
fn transfer_activity_is_idempotent_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let event = TransferLifecycleEvent {
        schema_version: 1,
        event_id: "event-1".into(),
        event_name: "completion".into(),
        transfer_id: "transfer-1".into(),
        sequence: 2,
        occurred_at_ms: 42,
        attempt: 1,
        payload: Some(serde_json::json!({
            "total_bytes": 12,
            "source_path": "/secret/private.txt",
            "token": "do-not-store",
        })),
    };
    {
        let storage = Storage::open(dir.path()).unwrap();
        storage.record_transfer_activity(&event).unwrap();
        storage.record_transfer_activity(&event).unwrap();
        assert_eq!(storage.list_transfer_activity(10).unwrap().len(), 1);
    }
    let storage = Storage::open(dir.path()).unwrap();
    let rows = storage.list_transfer_activity(10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].event_id, "event-1");
    assert_eq!(
        rows[0].payload_json.as_deref(),
        Some(r#"{"total_bytes":12}"#)
    );
}

#[test]
fn transfer_activity_records_direction_and_clear_is_non_destructive() {
    let storage = Storage::memory().unwrap();
    // Inbound lifecycle event (no direction marker → defaults inbound).
    storage
        .record_transfer_activity(&TransferLifecycleEvent {
            schema_version: 1,
            event_id: "evt-in".into(),
            event_name: "completion".into(),
            transfer_id: "t-in".into(),
            sequence: 0,
            occurred_at_ms: 10,
            attempt: 1,
            payload: Some(serde_json::json!({ "total_bytes": 4 })),
        })
        .unwrap();
    // Outbound lifecycle event produced by the blob provider consumer.
    storage
        .record_transfer_activity(&TransferLifecycleEvent {
            schema_version: 1,
            event_id: "evt-out".into(),
            event_name: "completion".into(),
            transfer_id: "t-out".into(),
            sequence: 0,
            occurred_at_ms: 20,
            attempt: 1,
            payload: Some(serde_json::json!({ "direction": "outbound" })),
        })
        .unwrap();
    // A hostile payload cannot smuggle an unexpected direction.
    storage
        .record_transfer_activity(&TransferLifecycleEvent {
            schema_version: 1,
            event_id: "evt-evil".into(),
            event_name: "completion".into(),
            transfer_id: "t-evil".into(),
            sequence: 0,
            occurred_at_ms: 30,
            attempt: 1,
            payload: Some(serde_json::json!({ "direction": "../../etc" })),
        })
        .unwrap();

    let rows = storage.list_transfer_activity(10).unwrap();
    let by_id = |id: &str| rows.iter().find(|r| r.event_id == id).unwrap();
    assert_eq!(by_id("evt-in").direction, "inbound");
    assert_eq!(by_id("evt-out").direction, "outbound");
    assert_eq!(by_id("evt-evil").direction, "inbound");

    // Seed a shared file, a permission grant, and a queued download, then
    // clear the activity projection. Clear History must not touch them.
    storage
        .put_file_object("hash", 10, "text/plain", "x.txt", b"")
        .unwrap();
    storage
        .upsert_shared_file("hash", "owner", "meta", "x.txt", None, true)
        .unwrap();
    storage
        .grant_permission("hash", "owner", "peer", "read", None)
        .unwrap();
    storage.create_download("hash", "peer", 10).unwrap();

    assert_eq!(storage.clear_transfer_activity().unwrap(), 3);
    assert!(storage.list_transfer_activity(10).unwrap().is_empty());
    assert!(storage.get_shared_file("owner", "hash").unwrap().is_some());
    assert_eq!(
        storage.list_permissions_for_grantee("peer").unwrap().len(),
        1
    );
    assert_eq!(storage.list_downloads().unwrap().len(), 1);
}

#[test]
fn activity_retention_prunes_old_rows() {
    let storage = Storage::memory().unwrap();
    for (id, occurred_at_ms) in [("old", 10), ("new", 20)] {
        storage
            .record_transfer_activity(&TransferLifecycleEvent {
                schema_version: 1,
                event_id: id.into(),
                event_name: "completion".into(),
                transfer_id: id.into(),
                sequence: 0,
                occurred_at_ms,
                attempt: 1,
                payload: None,
            })
            .unwrap();
    }
    assert_eq!(storage.prune_transfer_activity(20).unwrap(), 1);
    assert_eq!(
        storage.list_transfer_activity(10).unwrap()[0].event_id,
        "new"
    );
}

#[test]
fn deleting_shared_file_removes_grants_but_not_active_download() {
    let storage = Storage::memory().unwrap();
    storage
        .put_file_object("hash", 10, "text/plain", "x.txt", b"")
        .unwrap();
    storage
        .upsert_shared_file("hash", "owner", "meta", "x.txt", None, true)
        .unwrap();
    storage
        .grant_permission("hash", "owner", "peer", "read", None)
        .unwrap();
    let download = storage.create_download("hash", "peer", 10).unwrap();
    assert!(storage.delete_shared_file("hash", "owner").unwrap());
    assert!(storage
        .list_permissions_for_grantee("peer")
        .unwrap()
        .is_empty());
    assert_eq!(
        storage.get_download(download).unwrap().unwrap().state,
        "queued"
    );
}

#[test]
fn list_permissions_for_grantor_returns_only_my_grants() {
    let storage = Storage::memory().unwrap();
    storage
        .put_file_object("hash-a", 10, "text/plain", "a.txt", b"")
        .unwrap();
    storage
        .upsert_shared_file("hash-a", "owner", "meta-a", "a.txt", None, true)
        .unwrap();
    storage
        .grant_permission("hash-a", "owner", "peer-1", "read", None)
        .unwrap();
    storage
        .grant_permission("hash-a", "owner", "peer-2", "deny", None)
        .unwrap();
    // A grant made by someone else must never appear in my projection.
    storage
        .grant_permission("hash-a", "other-owner", "peer-1", "read", None)
        .unwrap();

    let mine = storage.list_permissions_for_grantor("owner").unwrap();
    assert_eq!(mine.len(), 2);
    assert!(mine.iter().all(|p| p.grantor_user_id == "owner"));
    let read_grantees: Vec<&str> = mine
        .iter()
        .filter(|p| p.permission == "read")
        .map(|p| p.grantee_user_id.as_str())
        .collect();
    assert_eq!(read_grantees, vec!["peer-1"]);

    let theirs = storage.list_permissions_for_grantor("other-owner").unwrap();
    assert_eq!(theirs.len(), 1);
    assert_eq!(theirs[0].grantee_user_id, "peer-1");
}

// ── Completed-download history (FS-15 Downloaded tab) ──────────────

fn insert_completed_download(
    storage: &Storage,
    hash: &str,
    filename: &str,
    mime: &str,
    peer: &str,
    total: u64,
    destination: Option<&str>,
) -> i64 {
    storage
        .put_file_object(hash, total, mime, filename, b"")
        .unwrap();
    let id = storage.create_download(hash, peer, total).unwrap();
    storage
        .set_download_paths(id, format!("/tmp/{hash}.part"), destination.unwrap_or(""))
        .unwrap();
    storage.complete_download(id, total).unwrap();
    id
}

#[test]
fn list_completed_downloads_is_newest_first_with_metadata() {
    let storage = Storage::memory().unwrap();
    let id1 = insert_completed_download(
        &storage,
        "hash-a",
        "report.pdf",
        "application/pdf",
        "peer-a",
        100,
        Some("/tmp/report.pdf"),
    );
    let id2 = insert_completed_download(
        &storage,
        "hash-b",
        "photo.png",
        "image/png",
        "peer-b",
        50,
        Some("/tmp/photo.png"),
    );

    // A non-complete row must not appear in completed history.
    storage
        .put_file_object("hash-c", 10, "text/plain", "pending.txt", b"")
        .unwrap();
    let pending = storage.create_download("hash-c", "peer-c", 10).unwrap();

    let rows = storage.list_completed_downloads().unwrap();
    assert_eq!(rows.len(), 2);
    // Newest first: id2 was completed after id1.
    assert_eq!(rows[0].id, id2);
    assert_eq!(rows[0].display_filename, "photo.png");
    assert_eq!(rows[0].mime_type, "image/png");
    assert_eq!(rows[0].remote_peer, "peer-b");
    assert_eq!(rows[0].total_bytes, 50);
    assert_eq!(rows[0].destination_path.as_deref(), Some("/tmp/photo.png"));
    assert_eq!(rows[1].id, id1);
    assert_eq!(rows[1].display_filename, "report.pdf");

    // Pending row untouched.
    assert_eq!(
        storage.get_download(pending).unwrap().unwrap().state,
        "queued"
    );
}

#[test]
fn delete_download_history_removes_record_but_never_the_file() {
    let storage = Storage::memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("keep.bin");
    std::fs::write(&destination, b"user file bytes").unwrap();
    let id = insert_completed_download(
        &storage,
        "hash-keep",
        "keep.bin",
        "application/octet-stream",
        "peer-a",
        10,
        Some(destination.to_str().unwrap()),
    );
    assert_eq!(storage.list_completed_downloads().unwrap().len(), 1);

    assert!(storage.delete_download_history(id).unwrap());
    assert!(storage.list_completed_downloads().unwrap().is_empty());

    // The on-disk file survives history removal untouched.
    assert!(destination.is_file());
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "user file bytes"
    );
}

#[test]
fn delete_download_history_refuses_active_rows() {
    let storage = Storage::memory().unwrap();
    storage
        .put_file_object("hash-active", 10, "text/plain", "active.bin", b"")
        .unwrap();
    let id = storage
        .create_download("hash-active", "peer-a", 10)
        .unwrap();
    storage
        .update_download_progress(id, 5, "downloading")
        .unwrap();

    // Active rows are not history; deletion is refused so the transfer
    // state machine is never disturbed.
    assert!(!storage.delete_download_history(id).unwrap());
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        "downloading"
    );
}

#[test]
fn completed_downloads_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let id;
    {
        let storage = Storage::open(dir.path()).unwrap();
        id = insert_completed_download(
            &storage,
            "hash-restart",
            "restart.bin",
            "application/octet-stream",
            "peer-z",
            7,
            Some("/tmp/restart.bin"),
        );
    }
    let storage = Storage::open(dir.path()).unwrap();
    let rows = storage.list_completed_downloads().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert_eq!(rows[0].display_filename, "restart.bin");
    assert_eq!(rows[0].content_hash, "hash-restart");
}

// ── Sharing Summary projections (FS-13) ─────────────────────────────

fn insert_download_with_state(storage: &Storage, hash: &str, peer: &str, state: &str) -> i64 {
    storage
        .put_file_object(hash, 100, "application/octet-stream", "f.bin", b"")
        .unwrap();
    let id = storage.create_download(hash, peer, 100).unwrap();
    storage.update_download_progress(id, 100, state).unwrap();
    id
}

#[test]
fn list_downloads_returns_all_states_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).unwrap();
    let id1 = insert_download_with_state(&storage, "hash-q", "peer-a", "queued");
    let id2 = insert_download_with_state(&storage, "hash-d", "peer-b", "downloading");
    let id3 = insert_download_with_state(&storage, "hash-c", "peer-c", "complete");

    let rows = storage.list_downloads().unwrap();
    assert_eq!(rows.len(), 3);
    // Oldest first with stable id tiebreak.
    assert_eq!(rows[0].id, id1);
    assert_eq!(rows[1].id, id2);
    assert_eq!(rows[2].id, id3);
    assert_eq!(rows[0].state, "queued");
    assert_eq!(rows[1].state, "downloading");
    assert_eq!(rows[2].state, "complete");
}

#[test]
fn list_shared_peer_ids_is_distinct_and_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).unwrap();
    // Grant two files to peer-a and one to peer-b (from the local profile).
    for hash in ["hash-1", "hash-2", "hash-3", "hash-4"] {
        storage
            .put_file_object(hash, 10, "application/octet-stream", "f.bin", b"")
            .unwrap();
    }
    storage
        .grant_permission("hash-1", "local", "peer-a", "read", None)
        .unwrap();
    storage
        .grant_permission("hash-2", "local", "peer-a", "read", None)
        .unwrap();
    storage
        .grant_permission("hash-3", "local", "peer-b", "read", None)
        .unwrap();
    // Another profile's grants must not leak into the local list.
    storage
        .grant_permission("hash-4", "other", "peer-c", "read", None)
        .unwrap();

    let peers = storage.list_shared_peer_ids("local").unwrap();
    assert_eq!(peers, vec!["peer-a".to_string(), "peer-b".to_string()]);

    // No grants → empty list (distinct from unknown/loading state).
    assert!(storage.list_shared_peer_ids("nobody").unwrap().is_empty());
}

#[test]
fn summary_projection_counts_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let storage = Storage::open(dir.path()).unwrap();
        insert_download_with_state(&storage, "hash-a", "peer-a", "complete");
        insert_download_with_state(&storage, "hash-b", "peer-b", "downloading");
        insert_download_with_state(&storage, "hash-c", "peer-c", "failed");
        storage
            .grant_permission("hash-a", "local", "peer-a", "read", None)
            .unwrap();
    }
    let storage = Storage::open(dir.path()).unwrap();
    let downloads = storage.list_downloads().unwrap();
    let peers = storage.list_shared_peer_ids("local").unwrap();
    assert_eq!(downloads.len(), 3);
    assert_eq!(peers.len(), 1);
}

// ── Permission expiry (FS-20 security hardening) ─────────────────────

#[test]
fn permission_is_active_at_respects_expiry_boundary() {
    let make = |expires: Option<u64>| crate::storage::SharedFilePermission {
        content_hash: "hash".into(),
        grantor_user_id: "owner".into(),
        grantee_user_id: "peer".into(),
        permission: "read".into(),
        created_at_ms: 1000,
        expires_at_ms: expires,
    };
    // No expiry → always active.
    assert!(make(None).is_active_at(0));
    assert!(make(None).is_active_at(u64::MAX));
    // Strictly in the future → active.
    assert!(make(Some(2000)).is_active_at(1999));
    // At the exact boundary → inactive (expiry is exclusive).
    assert!(!make(Some(2000)).is_active_at(2000));
    // In the past → inactive.
    assert!(!make(Some(2000)).is_active_at(2001));
}

#[test]
fn catalogue_entries_for_peer_hides_file_with_only_expired_grants() {
    let storage = Storage::memory().unwrap();
    let peer = iroh::SecretKey::generate().public();
    let now = now_ms();
    storage
        .put_file_object("hash-x", 10, "text/plain", "x.txt", b"")
        .unwrap();
    storage
        .upsert_shared_file("hash-x", "owner", "meta-x", "x.txt", None, true)
        .unwrap();
    // Grant is expired — must not make the file visible.
    storage
        .grant_permission("hash-x", "owner", &peer.to_string(), "read", Some(now - 1))
        .unwrap();

    let friends = crate::friends::FriendsStore::default();
    let view = storage
        .catalogue_entries_for_peer("owner", &peer, &friends)
        .unwrap();
    assert!(
        view.files.is_empty(),
        "expired read grant must not expose the file to the requester"
    );
}

#[test]
fn catalogue_entries_for_peer_still_lists_active_grant() {
    let storage = Storage::memory().unwrap();
    let peer = iroh::SecretKey::generate().public();
    let now = now_ms();
    storage
        .put_file_object("hash-y", 10, "text/plain", "y.txt", b"")
        .unwrap();
    storage
        .upsert_shared_file("hash-y", "owner", "meta-y", "y.txt", None, true)
        .unwrap();
    storage
        .grant_permission(
            "hash-y",
            "owner",
            &peer.to_string(),
            "read",
            Some(now + 60_000),
        )
        .unwrap();

    let friends = crate::friends::FriendsStore::default();
    let view = storage
        .catalogue_entries_for_peer("owner", &peer, &friends)
        .unwrap();
    assert_eq!(view.files.len(), 1);
    assert_eq!(view.files[0].content_hash, "hash-y");
}

/// A shared-file row whose content record (file_objects) is missing is
/// corrupt/stale: it must NOT be advertised with guessed metadata (size
/// 0, empty MIME) in a signed catalogue (BORU-AUDIT-07).  The orphan
/// state is simulated by dropping the content record after the offer is
/// created (foreign keys off), matching what a crash or legacy schema
/// can leave behind.
#[test]
fn catalogue_entries_for_peer_skips_missing_file_object() {
    let storage = Storage::memory().unwrap();
    let peer = iroh::SecretKey::generate().public();
    // Create the orphan offer normally, then lose its content record.
    storage
        .put_file_object("orphan-hash", 10, "text/plain", "orphan.txt", b"orphan")
        .unwrap();
    storage
        .upsert_shared_file(
            "orphan-hash",
            "owner",
            "meta-orphan",
            "orphan.txt",
            None,
            true,
        )
        .unwrap();
    storage
        .with_conn(|conn| {
            conn.execute_batch("PRAGMA foreign_keys = OFF;")
                .map_err(|e| anyhow::anyhow!("disable fk: {e}"))?;
            conn.execute(
                "DELETE FROM file_objects WHERE content_hash = 'orphan-hash'",
                [],
            )
            .map_err(|e| anyhow::anyhow!("delete file object: {e}"))?;
            conn.execute_batch("PRAGMA foreign_keys = ON;")
                .map_err(|e| anyhow::anyhow!("re-enable fk: {e}"))?;
            Ok(())
        })
        .unwrap();
    // A healthy file alongside it stays visible (granted to the peer).
    storage
        .put_file_object("hash-z", 10, "text/plain", "z.txt", b"")
        .unwrap();
    storage
        .upsert_shared_file("hash-z", "owner", "meta-z", "z.txt", None, true)
        .unwrap();
    storage
        .grant_permission(
            "hash-z",
            "owner",
            &peer.to_string(),
            "read",
            Some(now_ms() + 60_000),
        )
        .unwrap();

    let friends = crate::friends::FriendsStore::default();
    let view = storage
        .catalogue_entries_for_peer("owner", &peer, &friends)
        .unwrap();
    assert_eq!(
        view.files.len(),
        1,
        "the orphan record must be skipped, leaving only the healthy file"
    );
    assert_eq!(view.files[0].content_hash, "hash-z");
}

// ── Async facade (BORU-AUDIT-18) ─────────────────────────────────

/// A slow simulated SQLite query must not stall an independent Tokio
/// timer/network task: the query runs on the blocking pool, so a timer
/// fires long before the query completes.
#[cfg(feature = "net")]
#[tokio::test]
async fn slow_db_op_does_not_stall_independent_timer() {
    let storage = Storage::memory().unwrap();

    // Start a deliberately slow blocking "query" (300ms of fake work).
    let slow_task = tokio::spawn({
        let storage = storage.clone();
        async move {
            storage
                .run_blocking("test.slow_query", |_| {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    Ok::<_, anyhow::Error>(42u64)
                })
                .await
        }
    });

    // Give the blocking op a head start so it is genuinely in flight.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // An independent timer must fire well before the 300ms op completes.
    // If the facade ran the query on this worker thread, the timeout
    // would not be polled until ~300ms and this would panic.
    let started = std::time::Instant::now();
    tokio::time::timeout(std::time::Duration::from_millis(100), async {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    })
    .await
    .expect("independent timer stalled behind slow DB op");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(80),
        "timer took {:?} — worker thread was blocked by the DB op",
        started.elapsed()
    );

    let value = slow_task.await.unwrap().unwrap();
    assert_eq!(value, 42);
}

/// Concurrent repository requests through the async facade are
/// serialized safely and every request returns correct results.
#[cfg(feature = "net")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_facade_requests_serialize_and_return_correct_results() {
    let storage = Storage::memory().unwrap();
    let mut handles = Vec::new();
    for i in 0..16u64 {
        let storage = storage.clone();
        handles.push(tokio::spawn(async move {
            let hash = format!("concurrent-hash-{i}");
            let result = storage
                .run_blocking("test.concurrent_write", move |s| {
                    s.put_file_object(&hash, i, "text/plain", "f.txt", b"data")
                        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
                    Ok::<_, anyhow::Error>(())
                })
                .await;
            assert!(result.is_ok(), "write {i} failed: {result:?}");
            i
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }
    // Every write landed with the right payload — serialization did not
    // lose or corrupt any request.
    for i in 0..16u64 {
        let hash = format!("concurrent-hash-{i}");
        let obj = storage
            .get_file_object(&hash)
            .expect("read failed")
            .expect("object missing");
        assert_eq!(obj.size, i, "concurrent write {i} had wrong payload");
    }
}

/// Shutdown while writes are queued is deterministic: queued writes
/// complete (they were admitted before close), and operations submitted
/// after shutdown fail fast instead of queueing forever.
#[cfg(feature = "net")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_flushes_queued_writes_and_rejects_new() {
    let storage = Storage::memory().unwrap();
    let mut handles = Vec::new();

    // Queue a burst of writes, some still in flight when shutdown hits.
    for i in 0..8u64 {
        let storage = storage.clone();
        handles.push(tokio::spawn(async move {
            let hash = format!("shutdown-hash-{i}");
            let result = storage
                .run_blocking("test.shutdown_write", move |s| {
                    std::thread::sleep(std::time::Duration::from_millis(10 + (i % 3) * 10));
                    s.put_file_object(&hash, i, "text/plain", "f.txt", b"data")
                        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
                    Ok::<_, anyhow::Error>(hash)
                })
                .await;
            (i, result)
        }));
    }

    // Let every task pass the admission check so all writes are
    // guaranteed to be in flight, then shut down mid-burst.
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    storage.shutdown().await;

    // Every admitted write completed — none hung, none was lost.
    for handle in handles {
        let (i, result) = handle.await.unwrap();
        assert!(result.is_ok(), "queued write {i} failed: {result:?}");
    }
    for i in 0..8u64 {
        let hash = format!("shutdown-hash-{i}");
        let obj = storage
            .get_file_object(&hash)
            .expect("read after shutdown failed")
            .expect("queued write vanished after shutdown");
        assert_eq!(obj.size, i);
    }

    // After shutdown, new operations fail fast instead of queueing.
    let err = storage
        .run_blocking("test.after_shutdown", |_| Ok::<_, anyhow::Error>(1u64))
        .await;
    assert!(err.is_err(), "operations after shutdown must fail fast");
    assert!(
        err.unwrap_err().to_string().contains("shut down"),
        "error should name the shutdown state"
    );
}

#[test]
fn reaction_state_is_durable_and_remove_wins_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let message_id = [0x11; 32];
    let actor = [0x22; 32];
    {
        let storage = Storage::open(dir.path()).unwrap();
        assert!(storage
            .apply_reaction_event(&ReactionEvent::add(message_id, actor, "👍"), 1)
            .unwrap());
        assert!(storage
            .apply_reaction_event(&ReactionEvent::remove(message_id, actor, "👍"), 2)
            .unwrap());
        assert!(!storage
            .apply_reaction_event(&ReactionEvent::add(message_id, actor, "👍"), 3)
            .unwrap());
    }
    let storage = Storage::open(dir.path()).unwrap();
    let state = storage.load_reaction_state().unwrap();
    assert!(!state.contains(&message_id, &actor, "👍"));
    assert!(state.is_removed(&message_id, &actor, "👍"));
}
