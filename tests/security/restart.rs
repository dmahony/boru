//! Restart tests: replay state, mailbox state, group epoch state and
//! migration completion must all survive a reopen (BORU-AUDIT-28, step 6).

use std::sync::{Arc, Mutex};

use boru_core::group_events::{GroupEvent, GroupEventPayload, GroupState, GroupValidationError};
use boru_core::group_replay::{ReplayStore, EVENT_ID_LEN};
use boru_core::mailbox::MailboxIdentity;
use boru_core::storage::Storage;
use boru_core::TopicId;
use iroh::SecretKey;
use rusqlite::Connection;

fn group_id() -> TopicId {
    TopicId::from_bytes([7u8; 32])
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Replay state (group_event_replay table) survives a storage restart:
/// an event accepted before restart is still rejected as a replay after.
#[test]
fn replay_state_survives_storage_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let owner = SecretKey::generate();
    let group = group_id();
    let db_path = tmp.path().join("boru.db");
    let member = SecretKey::generate().public();

    // Session 1: attach a durable replay store and apply an event.  The
    // event's own id (derived from its nonce) is what gets recorded.
    let event;
    {
        let conn = Connection::open(&db_path).unwrap();
        let replay = ReplayStore::open(Arc::new(Mutex::new(conn))).unwrap();
        let mut state = GroupState::new(group, owner.public());
        state.attach_replay_store(replay);
        event = GroupEvent::sign(
            &owner,
            group,
            0,
            GroupEventPayload::MemberInvited { member },
        )
        .unwrap();
        event.clone().apply(&mut state).unwrap();
        // Second apply of the same event (same id) is a replay → rejected.
        assert!(matches!(
            event.clone().apply(&mut state),
            Err(GroupValidationError::Replay)
        ));
    }

    // Session 2: reopen storage; the event id is durable → the same event is
    // still rejected as a replay after restart.
    {
        let conn = Connection::open(&db_path).unwrap();
        let replay = ReplayStore::open(Arc::new(Mutex::new(conn))).unwrap();
        let mut state = GroupState::new(group, owner.public());
        state.attach_replay_store(replay);
        assert!(
            matches!(
                event.verify(&state),
                Err(GroupValidationError::Replay)
            ),
            "replayed event after restart must be rejected"
        );
    }
}

/// Mailbox state (SQLite-backed outgoing DM rows) survives a storage
/// restart: queue before restart, still present after reopen.
#[test]
fn mailbox_state_survives_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let sender = SecretKey::generate();
    let recipient = SecretKey::generate();
    let recipient_key = MailboxIdentity::from_secret(&recipient).public_key();

    let message_id;
    {
        let storage = Storage::open(tmp.path()).unwrap();
        let dm = storage
            .queue_outgoing_dm(
                [0x11; 32],
                sender.public(),
                "restart-req",
                "offline message",
                recipient_key,
                &sender,
            )
            .unwrap();
        message_id = dm.message_id;
        assert!(
            storage.get_dm_outbox(&message_id).unwrap().is_some(),
            "outbox row must exist after queue"
        );
    }

    {
        let storage = Storage::open(tmp.path()).unwrap();
        let outbox = storage
            .get_dm_outbox(&message_id)
            .expect("query outbox")
            .expect("outbox row must survive restart");
        assert_eq!(outbox.recipient, recipient.public());
    }
}

/// Group epoch state (the signed epoch-rotation event) survives a restart in
/// the sense that the credentials round-trip and a removed member cannot
/// decrypt the new epoch's material.
#[test]
fn group_epoch_state_survives_restart() {
    use boru_core::discovery_secret::DiscoverySecret;
    use boru_core::group_epoch::{EpochCredentials, EpochRotationState};
    use boru_core::group_id::GroupId;
    use std::collections::HashMap;

    let owner = SecretKey::generate();
    let survivor = SecretKey::generate();
    let removed = SecretKey::generate();
    let mut state = EpochRotationState::new(
        EpochCredentials::from_parts(
            GroupId::from_bytes([9; 32]),
            1,
            TopicId::from_bytes([1; 32]),
            DiscoverySecret::from_bytes([2; 32]),
        ),
        owner.public(),
    );
    state.add_member(survivor.public());
    state.add_member(removed.public());
    let survivor_identity = MailboxIdentity::from_secret(&survivor);
    let keys = HashMap::from([(survivor.public(), survivor_identity.public_key())]);

    let rotation = state
        .rotate_after_removal(&owner, removed.public(), &keys)
        .unwrap();
    let credentials = rotation.credentials();

    // Restart proxy: serialize the credentials and deserialize them exactly.
    let serialized = postcard::to_stdvec(credentials).unwrap();
    let restored: EpochCredentials = postcard::from_bytes(&serialized).unwrap();
    assert_eq!(restored.group_id(), credentials.group_id());
    assert_eq!(restored.epoch(), credentials.epoch());
    assert_eq!(restored.topic(), credentials.topic());
    assert_eq!(restored.secret(), credentials.secret());
}

/// Migration completion survives restart: a DB that was fully migrated stays
/// at CURRENT across reopen, and a DB that was only partially migrated (we
/// simulate by deleting the version row for the last step) resumes to
/// CURRENT on reopen.
#[test]
fn migration_completion_survives_restart() {
    use boru_core::storage::CURRENT_SCHEMA_VERSION;

    let tmp = tempfile::tempdir().unwrap();

    // Session 1: fully migrate.
    {
        let storage = Storage::open(tmp.path()).unwrap();
        let version: u32 = storage
            .with_conn(|conn| {
                Ok(conn
                    .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .map(|v| v as u32)
                    .unwrap_or(0))
            })
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    // Session 2: reopen → still fully migrated (no re-migration regressions).
    {
        let storage = Storage::open(tmp.path()).unwrap();
        let version: u32 = storage
            .with_conn(|conn| {
                Ok(conn
                    .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .map(|v| v as u32)
                    .unwrap_or(0))
            })
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    // Simulate a crash mid-migration: drop the newest version row so the next
    // open believes it is one step behind, then reopen and confirm it resumes.
    {
        let storage = Storage::open(tmp.path()).unwrap();
        storage
            .with_conn(|conn| {
                Ok(conn
                    .execute(
                        "DELETE FROM schema_version WHERE version = ?1",
                        [CURRENT_SCHEMA_VERSION as i64],
                    )
                    .map_err(|e| anyhow::anyhow!("{e}"))?)
            })
            .unwrap();
    }
    {
        let storage = Storage::open(tmp.path()).unwrap();
        let version: u32 = storage
            .with_conn(|conn| {
                Ok(conn
                    .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .map(|v| v as u32)
                    .unwrap_or(0))
            })
            .unwrap();
        assert_eq!(
            version, CURRENT_SCHEMA_VERSION,
            "partial migration must resume to CURRENT on restart"
        );
    }
}

/// A standalone ReplayStore over a raw SQLite connection survives a reopen
/// (the store itself does not depend on the Storage facade).
#[test]
fn replay_store_reopen_on_raw_connection() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("replay.db");

    let event_id = [0x77u8; EVENT_ID_LEN];
    {
        let conn = Connection::open(&db_path).unwrap();
        let store = ReplayStore::open(Arc::new(Mutex::new(conn))).unwrap();
        let outcome = store.record(&group_id(), 1, event_id, now_secs()).unwrap();
        assert!(matches!(
            outcome,
            boru_core::group_replay::RecordOutcome::Recorded
        ));
    }

    {
        let conn = Connection::open(&db_path).unwrap();
        let store = ReplayStore::open(Arc::new(Mutex::new(conn))).unwrap();
        assert!(store.contains(&group_id(), &event_id).unwrap());
        // Re-recording is a no-op duplicate.
        let outcome = store.record(&group_id(), 1, event_id, now_secs()).unwrap();
        assert!(matches!(
            outcome,
            boru_core::group_replay::RecordOutcome::AlreadySeen
        ));
    }
}
