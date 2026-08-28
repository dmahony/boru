//! Persistent, bounded replay-marker store for group control events.
//!
//! # Why this exists (BORU-AUDIT-16)
//!
//! Before this module, group replay protection was a single in-memory
//! `HashSet` inside [`GroupState`](crate::group_events::GroupState): it
//! never survived a restart, it had no epoch index, and it grew without
//! bound for the lifetime of the process. An attacker could replay an
//! accepted membership event after a restart, and a long-running node
//! accumulated every event ID ever seen.
//!
//! This module provides the durable half of the fix:
//!
//! - Accepted event IDs are recorded in SQLite (`group_event_replay`),
//!   keyed by `(group_id, event_id)` and indexed by `(group_id, epoch)` so
//!   pruning is a cheap range delete.
//! - [`ReplayStore::record`](crate::group_replay::ReplayStore::record) is a single atomic `INSERT OR IGNORE` — the
//!   replay marker lands in the same transaction as the acceptance decision,
//!   so a concurrent duplicate arrival cannot both be accepted.
//! - [`ReplayStore::prune_older_than`](crate::group_replay::ReplayStore::prune_older_than) deletes expired epochs in bounded
//!   batches (never one unbounded statement).
//!
//! The in-memory hot cache in [`GroupState`](crate::group_events::GroupState)
//! remains as a fast path, but it is capped and is treated as an optimization
//! over this persisted state — never the sole authority.

use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};

use crate::TopicId;

/// Length of a group event ID (first 16 bytes of a BLAKE3 hash).
pub const EVENT_ID_LEN: usize = 16;

/// Maximum rows deleted by one [`ReplayStore::prune_older_than`] statement.
/// Pruning loops in batches of this size so a huge backlog is cleared
/// incrementally instead of one giant DELETE that stalls the writer.
pub const REPLAY_PRUNE_BATCH: usize = 500;

/// Schema for the replay-marker table.
///
/// The composite primary key `(group_id, event_id)` makes
/// [`ReplayStore::record`] an idempotent `INSERT OR IGNORE`, and the
/// secondary index on `(group_id, epoch)` makes epoch-window pruning a
/// targeted range delete.
const REPLAY_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS group_event_replay (
    group_id BLOB NOT NULL,
    epoch    INTEGER NOT NULL,
    event_id BLOB NOT NULL,
    seen_at  INTEGER NOT NULL,
    PRIMARY KEY (group_id, event_id)
);
CREATE INDEX IF NOT EXISTS idx_group_event_replay_group_epoch
    ON group_event_replay (group_id, epoch);
";

/// Outcome of a single [`ReplayStore::record`] attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordOutcome {
    /// The marker was newly inserted; the event is accepted.
    Recorded,
    /// The marker already existed; the event is a replay and must be rejected.
    AlreadySeen,
}

/// Errors from the replay-marker store.
#[derive(Debug)]
pub enum ReplayStoreError {
    /// Underlying SQLite failure.
    Db(rusqlite::Error),
    /// The shared connection lock was poisoned.
    Lock(String),
}

impl std::fmt::Display for ReplayStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayStoreError::Db(e) => write!(f, "replay store db error: {e}"),
            ReplayStoreError::Lock(e) => write!(f, "replay store lock error: {e}"),
        }
    }
}

impl std::error::Error for ReplayStoreError {}

/// SQLite-backed store of accepted group event IDs.
///
/// Thread-safe and cheap to clone (the connection is shared behind an
/// `Arc<Mutex<_>>`), so a single store can back many [`GroupState`](crate::group_events::GroupState)s.
#[derive(Clone, Debug)]
pub struct ReplayStore {
    conn: Arc<Mutex<Connection>>,
}

impl ReplayStore {
    /// Open a replay store over an existing SQLite connection.
    ///
    /// Creates the `group_event_replay` table and its epoch index if they do
    /// not already exist (idempotent).
    pub fn open(conn: Arc<Mutex<Connection>>) -> Result<Self, ReplayStoreError> {
        let store = Self { conn };
        {
            let guard = store
                .conn
                .lock()
                .map_err(|e| ReplayStoreError::Lock(e.to_string()))?;
            guard
                .execute_batch(REPLAY_SCHEMA)
                .map_err(ReplayStoreError::Db)?;
        }
        Ok(store)
    }

    /// Check whether `event_id` has already been accepted for `group_id`.
    ///
    /// Fail-closed: any database error is returned to the caller, which must
    /// treat it as "cannot prove the event is fresh" and reject it.
    pub fn contains(
        &self,
        group_id: &TopicId,
        event_id: &[u8; EVENT_ID_LEN],
    ) -> Result<bool, ReplayStoreError> {
        let guard = self
            .conn
            .lock()
            .map_err(|e| ReplayStoreError::Lock(e.to_string()))?;
        let mut stmt = guard
            .prepare("SELECT 1 FROM group_event_replay WHERE group_id = ?1 AND event_id = ?2")
            .map_err(ReplayStoreError::Db)?;
        let found = stmt
            .exists(params![group_id.as_bytes(), event_id.as_slice()])
            .map_err(ReplayStoreError::Db)?;
        Ok(found)
    }

    /// Atomically record an accepted event's marker.
    ///
    /// `INSERT OR IGNORE` is a single atomic statement, so two concurrent
    /// callers presenting the same event ID cannot both observe a fresh
    /// insert: exactly one gets [`RecordOutcome::Recorded`] and the other
    /// gets [`RecordOutcome::AlreadySeen`]. `seen_at` is Unix seconds.
    pub fn record(
        &self,
        group_id: &TopicId,
        epoch: u64,
        event_id: [u8; EVENT_ID_LEN],
        seen_at: u64,
    ) -> Result<RecordOutcome, ReplayStoreError> {
        let guard = self
            .conn
            .lock()
            .map_err(|e| ReplayStoreError::Lock(e.to_string()))?;
        let inserted = guard
            .execute(
                "INSERT OR IGNORE INTO group_event_replay (group_id, epoch, event_id, seen_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    group_id.as_bytes(),
                    epoch as i64,
                    event_id.as_slice(),
                    seen_at as i64
                ],
            )
            .map_err(ReplayStoreError::Db)?;
        Ok(if inserted == 1 {
            RecordOutcome::Recorded
        } else {
            RecordOutcome::AlreadySeen
        })
    }

    /// Remove replay markers for `group_id` whose epoch is strictly older
    /// than `min_epoch`, in bounded batches.
    ///
    /// Returns the total number of rows deleted. A replay of an event from a
    /// pruned epoch is still rejected by the epoch check in
    /// [`GroupEvent::verify`](crate::group_events::GroupEvent::verify), so
    /// pruning never re-opens the acceptance window for the current epoch.
    pub fn prune_older_than(
        &self,
        group_id: &TopicId,
        min_epoch: u64,
    ) -> Result<usize, ReplayStoreError> {
        let guard = self
            .conn
            .lock()
            .map_err(|e| ReplayStoreError::Lock(e.to_string()))?;
        let mut total = 0usize;
        loop {
            // SQLite does not allow LIMIT directly on DELETE; delete at most
            // REPLAY_PRUNE_BATCH rows by selecting their rowids first.
            let deleted = guard
                .execute(
                    "DELETE FROM group_event_replay
                     WHERE group_id = ?1 AND epoch < ?2
                       AND rowid IN (
                           SELECT rowid FROM group_event_replay
                           WHERE group_id = ?1 AND epoch < ?2
                           LIMIT ?3
                       )",
                    params![
                        group_id.as_bytes(),
                        min_epoch as i64,
                        REPLAY_PRUNE_BATCH as i64
                    ],
                )
                .map_err(ReplayStoreError::Db)?;
            total += deleted;
            if deleted < REPLAY_PRUNE_BATCH {
                break;
            }
        }
        Ok(total)
    }

    /// Number of retained markers for `group_id` (tests and diagnostics).
    pub fn count(&self, group_id: &TopicId) -> Result<usize, ReplayStoreError> {
        let guard = self
            .conn
            .lock()
            .map_err(|e| ReplayStoreError::Lock(e.to_string()))?;
        let count = guard
            .query_row(
                "SELECT COUNT(*) FROM group_event_replay WHERE group_id = ?1",
                params![group_id.as_bytes()],
                |row| row.get::<_, i64>(0),
            )
            .map_err(ReplayStoreError::Db)?;
        Ok(count as usize)
    }

    /// Highest epoch among retained markers for `group_id`, if any.
    pub fn max_epoch(&self, group_id: &TopicId) -> Result<Option<u64>, ReplayStoreError> {
        let guard = self
            .conn
            .lock()
            .map_err(|e| ReplayStoreError::Lock(e.to_string()))?;
        let max_epoch = guard
            .query_row(
                "SELECT MAX(epoch) FROM group_event_replay WHERE group_id = ?1",
                params![group_id.as_bytes()],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(ReplayStoreError::Db)?;
        Ok(max_epoch.map(|v| v as u64))
    }

    /// Expose the underlying connection (tests).
    #[doc(hidden)]
    pub fn connection(&self) -> &Arc<Mutex<Connection>> {
        &self.conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> ReplayStore {
        let conn = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        ReplayStore::open(conn).unwrap()
    }

    fn gid(tag: u8) -> TopicId {
        [tag; 32].into()
    }

    #[test]
    fn record_then_contains_is_true() {
        let s = store();
        let g = gid(7);
        let id = [0x42u8; EVENT_ID_LEN];
        assert_eq!(
            s.record(&g, 0, id, 1_700_000_000).unwrap(),
            RecordOutcome::Recorded
        );
        assert!(s.contains(&g, &id).unwrap());
        assert!(!s.contains(&g, &[0u8; EVENT_ID_LEN]).unwrap());
    }

    #[test]
    fn duplicate_record_returns_already_seen() {
        let s = store();
        let g = gid(7);
        let id = [0x42u8; EVENT_ID_LEN];
        assert_eq!(
            s.record(&g, 0, id, 1_700_000_000).unwrap(),
            RecordOutcome::Recorded
        );
        assert_eq!(
            s.record(&g, 0, id, 1_700_000_001).unwrap(),
            RecordOutcome::AlreadySeen
        );
    }

    #[test]
    fn markers_are_per_group() {
        let s = store();
        let id = [0x42u8; EVENT_ID_LEN];
        s.record(&gid(1), 0, id, 1_700_000_000).unwrap();
        assert!(!s.contains(&gid(2), &id).unwrap());
    }

    #[test]
    fn prune_removes_only_old_epochs() {
        let s = store();
        let g = gid(7);
        for epoch in 0..5u64 {
            let id = [epoch as u8; EVENT_ID_LEN];
            s.record(&g, epoch, id, 1_700_000_000).unwrap();
        }
        // Keep epochs >= 3; delete 0,1,2.
        let deleted = s.prune_older_than(&g, 3).unwrap();
        assert_eq!(deleted, 3);
        assert_eq!(s.count(&g).unwrap(), 2);
        assert_eq!(s.max_epoch(&g).unwrap(), Some(4));
        // Active epochs are untouched.
        assert!(s.contains(&g, &[3u8; EVENT_ID_LEN]).unwrap());
        assert!(s.contains(&g, &[4u8; EVENT_ID_LEN]).unwrap());
        assert!(!s.contains(&g, &[0u8; EVENT_ID_LEN]).unwrap());
    }

    #[test]
    fn prune_with_nothing_to_delete_is_zero() {
        let s = store();
        let g = gid(7);
        s.record(&g, 5, [5u8; EVENT_ID_LEN], 1_700_000_000).unwrap();
        assert_eq!(s.prune_older_than(&g, 0).unwrap(), 0);
    }

    #[test]
    fn prune_large_backlog_in_batches() {
        let s = store();
        let g = gid(7);
        // 2.5 batches worth of old markers plus one fresh.
        for i in 0..(REPLAY_PRUNE_BATCH * 2 + 50) {
            let mut id = [0u8; EVENT_ID_LEN];
            id[..8].copy_from_slice(&(i as u64).to_le_bytes());
            s.record(&g, 0, id, 1_700_000_000).unwrap();
        }
        let fresh = [0xFFu8; EVENT_ID_LEN];
        s.record(&g, 1, fresh, 1_700_000_000).unwrap();
        let deleted = s.prune_older_than(&g, 1).unwrap();
        assert_eq!(deleted, REPLAY_PRUNE_BATCH * 2 + 50);
        assert_eq!(s.count(&g).unwrap(), 1);
        assert!(s.contains(&g, &fresh).unwrap());
    }
}
