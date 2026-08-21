//! Cross-version profile/database upgrade fixtures.
//!
//! The SQL fixtures are intentionally compact reductions of historical schema
//! families rather than dumps of a developer profile.  Each contains only
//! deterministic synthetic bytes; `manifest.json` records the semantic values
//! the harness must preserve.  v1 is based on the v0.103.0 storage schema,
//! v13 on v0.108.0, and v23 is the immediately-pre-current family (the current
//! tree's v24-v26 migrations are deliberately pending).

use std::{fs, path::Path};

use rusqlite::{params, Connection};
use tempfile::TempDir;

use boru_core::storage::CURRENT_SCHEMA_VERSION;

const FIXTURE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/storage-upgrade"
);
const FAMILIES: &[&str] = &["v1", "v13", "v23"];

fn make_profile(family: &str) -> (TempDir, serde_json::Value) {
    let source = Path::new(FIXTURE_ROOT).join(family);
    let dir = tempfile::tempdir().expect("temporary profile");
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(source.join("manifest.json")).expect("fixture manifest"),
    )
    .expect("valid fixture manifest");

    let conn = Connection::open(dir.path().join("boru.db")).expect("fixture database");
    conn.execute_batch(&fs::read_to_string(source.join("fixture.sql")).expect("fixture SQL"))
        .expect("load fixture SQL");
    drop(conn);
    fs::copy(
        source.join("manifest.json"),
        dir.path().join("manifest.json"),
    )
    .expect("copy manifest into profile");
    (dir, manifest)
}

fn db_version(path: &Path) -> u32 {
    let conn = Connection::open(path).expect("open database for inspection");
    conn.query_row("SELECT MAX(version) FROM schema_version", [], |row| {
        row.get(0)
    })
    .expect("schema version")
}

fn assert_integrity(path: &Path) {
    let conn = Connection::open(path).expect("open database for integrity check");
    let result: String = conn
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .expect("integrity check pragma");
    assert_eq!(result, "ok", "fixture database integrity check");
}

fn assert_semantics(path: &Path, manifest: &serde_json::Value) {
    let conn = Connection::open(path).expect("open migrated database");
    let expected_messages = manifest["message_count"].as_i64().unwrap() as usize;
    let actual_messages: usize = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM inbox) + (SELECT COUNT(*) FROM chat_messages)",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("message count") as usize;
    assert_eq!(
        actual_messages, expected_messages,
        "message count survived upgrade"
    );

    let expected_files = manifest["file_metadata"].as_array().unwrap().len();
    let actual_files: usize = conn
        .query_row("SELECT COUNT(*) FROM shared_files", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("file metadata count") as usize;
    assert_eq!(
        actual_files, expected_files,
        "file metadata survived upgrade"
    );

    // These rows are the durable identity/friend/conversation/room projection
    // in the fixture manifest.  They must remain byte-for-byte unchanged even
    // though the current Storage layer does not rewrite sidecar projections.
    assert!(!manifest["profile_public_id"].as_str().unwrap().is_empty());
    assert!(!manifest["friend_ids"].as_array().unwrap().is_empty());
    assert!(!manifest["conversation_ids"].as_array().unwrap().is_empty());
    assert!(!manifest["room_names"].as_array().unwrap().is_empty());

    let expected_hashes = manifest["message_hashes"].as_array().unwrap();
    for hash in expected_hashes {
        let hex_hash = hash.as_str().unwrap();
        let hash_bytes = hex::decode(hex_hash).expect("fixture message hash");
        let found: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM (
                    SELECT msg_id AS value FROM inbox
                    UNION ALL
                    SELECT msg_hash AS value FROM chat_messages
                ) WHERE value = ?1",
                params![hash_bytes],
                |row| row.get(0),
            )
            .expect("message hash lookup");
        assert_eq!(found, 1, "message hash survived upgrade: {hex_hash}");
    }
}

#[test]
fn historical_fixtures_upgrade_reopen_and_preserve_semantics() {
    for family in FAMILIES {
        let (profile, manifest) = make_profile(family);
        let db = profile.path().join("boru.db");
        let manifest_before =
            fs::read(profile.path().join("manifest.json")).expect("manifest bytes");
        assert_eq!(db_version(&db), family[1..].parse::<u32>().unwrap());

        let storage = boru_core::storage::Storage::open(profile.path()).expect("upgrade fixture");
        drop(storage);
        assert_eq!(
            db_version(&db),
            CURRENT_SCHEMA_VERSION,
            "{family} reaches current schema"
        );
        assert_integrity(&db);
        assert_semantics(&db, &manifest);

        // Reopen is the idempotence check and catches migrations that only
        // happen to work in the first process.
        let storage =
            boru_core::storage::Storage::open(profile.path()).expect("reopen upgraded fixture");
        drop(storage);
        assert_eq!(
            db_version(&db),
            CURRENT_SCHEMA_VERSION,
            "{family} remains current"
        );
        assert_integrity(&db);
        assert_semantics(&db, &manifest);
        assert_eq!(
            fs::read(profile.path().join("manifest.json")).unwrap(),
            manifest_before
        );
    }
}

#[test]
fn future_schema_rejection_is_non_destructive_and_backup_restore_is_safe() {
    let (profile, manifest) = make_profile("v23");
    let db = profile.path().join("boru.db");
    let backup = profile.path().join("boru.db.backup");

    // Complete the normal upgrade before taking a backup, matching the safe
    // ordering used by the application: backup first, then any risky open.
    boru_core::storage::Storage::open(profile.path()).expect("initial upgrade");
    fs::copy(&db, &backup).expect("safe backup");
    {
        let conn = Connection::open(&db).expect("inject future version");
        conn.execute(
            "INSERT INTO schema_version(version, applied_at_ms) VALUES (?1, ?2)",
            params![CURRENT_SCHEMA_VERSION + 1, 0i64],
        )
        .expect("future schema marker");
    }
    let error = match boru_core::storage::Storage::open(profile.path()) {
        Ok(_) => panic!("future schema rejected"),
        Err(error) => error,
    };
    let text = error.to_string();
    assert!(
        text.contains("newer version"),
        "explicit future-schema error: {text}"
    );
    assert_eq!(
        db_version(&db),
        CURRENT_SCHEMA_VERSION + 1,
        "failed open did not rewrite future DB"
    );

    // Do not let a journal from the rejected open overlay the restored main
    // database file.
    let _ = fs::remove_file(db.with_file_name("boru.db-wal"));
    let _ = fs::remove_file(db.with_file_name("boru.db-shm"));
    fs::copy(&backup, &db).expect("restore backup");
    let restored = boru_core::storage::Storage::open(profile.path()).expect("open restored backup");
    drop(restored);
    assert_eq!(db_version(&db), CURRENT_SCHEMA_VERSION);
    assert_integrity(&db);
    assert_semantics(&db, &manifest);
}
