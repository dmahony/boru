//! A migration's schema changes and version marker must commit together.
use boru_core::storage::Storage;
use rusqlite::Connection;

#[test]
fn failed_version_marker_rolls_back_schema_and_allows_retry() {
    let dir = tempfile::tempdir().unwrap();
    drop(Storage::open(dir.path()).unwrap());
    let path = dir.path().join("boru.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TABLE room_authorization_state;
         DROP TABLE room_authorization_events;
         DELETE FROM schema_version WHERE version = 26;
         CREATE TRIGGER reject_version BEFORE INSERT ON schema_version
         WHEN NEW.version = 26 BEGIN SELECT RAISE(ABORT, 'injected marker failure'); END;",
    )
    .unwrap();
    drop(conn);

    assert!(Storage::open(dir.path()).is_err());
    let conn = Connection::open(&path).unwrap();
    let tables: u32 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'
         AND name IN ('room_authorization_state', 'room_authorization_events')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        tables, 0,
        "failed version marker must roll back both new tables"
    );
    let version: u32 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, 25);
    conn.execute_batch("DROP TRIGGER reject_version").unwrap();
    drop(conn);

    drop(Storage::open(dir.path()).unwrap());
    let conn = Connection::open(&path).unwrap();
    let version: u32 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, 26);
    let tables: u32 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'
         AND name IN ('room_authorization_state', 'room_authorization_events')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 2);
}
