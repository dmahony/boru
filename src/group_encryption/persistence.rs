//! Persistence for per-group encryption state.
//!
//! Saves and loads [`GroupEncryptionState`](crate::group_encryption::encryption_state::GroupEncryptionState) (a serialised [`GroupState`](crate::group_events::GroupState) from
//! p2panda-encryption) to/from the `group_encryption_state` SQLite table.
//!
//! # Table schema (created by migration v13)
//!
//! ```sql
//! CREATE TABLE group_encryption_state (
//!     group_id   BLOB PRIMARY KEY,
//!     state      BLOB NOT NULL,
//!     updated_at INTEGER NOT NULL
//! );
//! ```
//!
//! The `state` column stores a postcard-encoded, **versioned envelope**
//! (`StoredGroupStateV1`) wrapping a [`GroupEncryptionState`](crate::group_encryption::encryption_state::GroupEncryptionState) blob.
//! `updated_at` is a Unix-epoch milliseconds timestamp set on every write.
//!
//! # Fail-closed loading
//!
//! `load_group_state` distinguishes four conditions:
//!
//! - [`GroupStateLoadError::Missing`](crate::group_encryption::persistence::GroupStateLoadError::Missing) — no row exists. This is the **only**
//!   condition that permits fresh initialization (a genuinely new group).
//! - [`GroupStateLoadError::Corrupt`](crate::group_encryption::persistence::GroupStateLoadError::Corrupt) — a row exists but cannot be decoded or
//!   fails invariant validation. The raw record is **never overwritten**
//!   automatically; it stays in the table until an explicit user-approved
//!   reset moves it to the quarantine table (`quarantine_group_state`).
//! - [`GroupStateLoadError::UnsupportedVersion`](crate::group_encryption::persistence::GroupStateLoadError::UnsupportedVersion) — the row decodes but uses a
//!   format version this build cannot load (including the legacy pre-version
//!   raw blob, version 0). Route through migration rather than treating it as
//!   missing or corrupt.
//! - [`GroupStateLoadError::Io`](crate::group_encryption::persistence::GroupStateLoadError::Io) — an underlying database failure.

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use p2panda_encryption::message_scheme::group::MessageGroup;
use serde::{Deserialize, Serialize};

use crate::group_id::GroupId;

use super::encryption_state::GroupEncryptionState;
use super::membership::MemberRole;
use super::types::PeerId;

/// Current version of the on-disk group-state envelope.
///
/// Bump this only when the persisted format changes incompatibly; older
/// versions are reported as [`GroupStateLoadError::UnsupportedVersion`] so
/// they can be routed through migration instead of being mistaken for
/// corruption or missing state.
pub const GROUP_STATE_STORAGE_VERSION: u32 = 1;

/// Versioned envelope wrapping a persisted [`GroupEncryptionState`].
///
/// `version` is the first serialized field so the version can be read without
/// decoding the (potentially corrupt) payload.  `checksum` is a BLAKE3 hash
/// of every byte preceding it in the stored record — i.e. of `version` plus
/// the serialised `state` — so any single-bit corruption of the raw record is
/// detected even when it falls inside a raw key byte that postcard would
/// otherwise tolerate.
#[derive(Serialize, Deserialize)]
struct StoredGroupStateV1 {
    /// Storage format version ([`GROUP_STATE_STORAGE_VERSION`]).
    version: u32,
    /// The serialised p2panda group state.
    state: GroupEncryptionState,
    /// BLAKE3-256 of the raw bytes preceding this field.
    checksum: [u8; 32],
}

/// Encode a group state into the current versioned storage envelope.
///
/// The blob layout is exactly `[version varint][serialised state][32-byte
/// BLAKE3 checksum]`, matching the field order of [`StoredGroupStateV1`]. The
/// checksum covers the raw bytes of `version` + serialised `state` (everything
/// preceding the checksum), so verification on load does not depend on the
/// (non-canonical) map ordering inside the state.
fn encode_envelope(state: &GroupEncryptionState) -> Result<Vec<u8>, postcard::Error> {
    let version_bytes = postcard::to_stdvec(&GROUP_STATE_STORAGE_VERSION)?;
    let state_bytes = postcard::to_stdvec(state)?;
    let mut body = version_bytes;
    body.extend_from_slice(&state_bytes);
    let checksum = *blake3::hash(&body).as_bytes();
    body.extend_from_slice(&checksum);
    Ok(body)
}

/// Errors that can occur while loading a group's encryption state.
///
/// The caller must treat every variant except [`Self::Missing`] as a
/// fail-closed condition: fresh initialization is only permitted when no
/// saved state exists at all.
#[derive(Debug)]
pub enum GroupStateLoadError {
    /// No saved state for this group. The ONLY condition that permits fresh
    /// initialization (a genuinely new group or an explicit reset).
    Missing,
    /// A state record exists but cannot be deserialized or fails invariant
    /// validation. The raw record is preserved in the database — it is never
    /// overwritten automatically and must be quarantined by an explicit
    /// user-approved reset before it can be replaced.
    Corrupt {
        /// The group whose state is corrupt.
        group_id: GroupId,
        /// Human-readable reason (decode/validation failure). Never includes
        /// secret key material.
        reason: String,
    },
    /// The stored state uses a format version this build cannot load. This is
    /// distinct from corruption: route through the migration path. Version 0
    /// is the legacy raw (unversioned) blob written before versioning.
    UnsupportedVersion {
        /// The group whose state has an unsupported version.
        group_id: GroupId,
        /// The stored format version.
        version: u32,
    },
    /// Underlying database / I/O failure.
    Io(rusqlite::Error),
}

impl std::fmt::Display for GroupStateLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GroupStateLoadError::Missing => {
                write!(f, "no saved encryption state for group (fresh initialization permitted)")
            }
            GroupStateLoadError::Corrupt { group_id, reason } => write!(
                f,
                "encrypted group state for {group_id:?} is corrupt and needs recovery: {reason}"
            ),
            GroupStateLoadError::UnsupportedVersion { group_id, version } => write!(
                f,
                "encrypted group state for {group_id:?} uses unsupported format version {version} (migration required)"
            ),
            GroupStateLoadError::Io(e) => write!(f, "database error loading group state: {e}"),
        }
    }
}

impl std::error::Error for GroupStateLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GroupStateLoadError::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// Save (insert or update) the encryption state for a group.
///
/// Serialises `state` with postcard inside a versioned envelope and writes it
/// into the `group_encryption_state` table.  Uses INSERT OR REPLACE so
/// repeated saves for the same group are always idempotent.  The row's
/// optimistic-concurrency `version` column (if present) is preserved on
/// update; use [`save_group_state_and_roles`] for transactional writes that
/// also bump the version.
pub fn save_group_state(
    conn: &Connection,
    group_id: &GroupId,
    state: &GroupEncryptionState,
) -> rusqlite::Result<()> {
    ensure_version_column(conn)?;
    let blob =
        encode_envelope(state).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    conn.execute(
        "INSERT INTO group_encryption_state (group_id, state, updated_at, version) VALUES (?1, ?2, ?3, 1) \
         ON CONFLICT(group_id) DO UPDATE SET state=excluded.state, updated_at=excluded.updated_at",
        params![group_id.as_bytes().as_slice(), blob, now],
    )?;
    Ok(())
}

// ── Transactional group-state writes (BORU-AUDIT-09) ──────────────────
//
// Membership/role changes and the encrypted group state must commit as ONE
// logical transaction.  `save_group_state_and_roles` owns that boundary:
// it starts a single SQLite transaction, performs an optimistic-concurrency
// version check against the caller's `expected_version` (so two concurrent
// epoch operations cannot both commit from the same prior state), writes the
// role mirror, writes the new encrypted state, and commits.  On any failure
// the transaction rolls back completely — no partial membership/crypto state
// can leak to disk.

/// Deterministic failures used to verify group-auth transaction rollback.
///
/// Mirrors the [`crate::storage::OutgoingDmFault`] pattern: tests inject a
/// fault at a precise point inside the transaction and assert that *nothing*
/// was committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupAuthFault {
    /// Fail after the membership/roles row is written but before the
    /// crypto-state row is written.
    AfterMembershipWrite,
    /// Fail after the crypto-state row is written but before commit.
    AfterCryptoStateWrite,
}

/// Errors from a transactional group-auth write.
#[derive(Debug)]
pub enum GroupAuthTxError {
    /// The stored version does not match the caller's expected version —
    /// a concurrent operation committed first.  The caller must reload
    /// authoritative state and retry (or surface the conflict).
    VersionConflict {
        /// The version the caller believed was current.
        expected: u64,
        /// The version actually stored when the transaction began.
        current: u64,
    },
    /// Underlying database failure.
    Io(rusqlite::Error),
}

impl std::fmt::Display for GroupAuthTxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GroupAuthTxError::VersionConflict { expected, current } => write!(
                f,
                "group auth version conflict: expected {expected}, stored {current} (concurrent mutation)"
            ),
            GroupAuthTxError::Io(e) => write!(f, "group auth database error: {e}"),
        }
    }
}

impl std::error::Error for GroupAuthTxError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GroupAuthTxError::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// Ensure the `group_encryption_state` table carries the optimistic-
/// concurrency `version` column.  Older databases created by migration v13
/// lack the column; adding it lazily (idempotently) keeps the transaction
/// layer working without a full schema migration.  The column defaults to 0
/// for pre-existing rows.
fn ensure_version_column(conn: &Connection) -> rusqlite::Result<()> {
    match conn.execute_batch(
        "ALTER TABLE group_encryption_state ADD COLUMN version INTEGER NOT NULL DEFAULT 0;",
    ) {
        Ok(()) => Ok(()),
        // Already present (new DBs or repeated calls) — harmless.
        Err(e) if e.to_string().contains("duplicate column") => Ok(()),
        Err(e) => Err(e),
    }
}

/// Read the current optimistic-concurrency version for a group (0 when the
/// group has no persisted state row yet).
pub fn load_group_version(conn: &Connection, group_id: &GroupId) -> Result<u64, GroupAuthTxError> {
    // Older DBs created by migration v13 lack the version column; add it
    // lazily so the read is safe on both old and new schemas.
    ensure_version_column(conn).map_err(GroupAuthTxError::Io)?;
    let version: i64 = conn
        .query_row(
            "SELECT version FROM group_encryption_state WHERE group_id = ?1",
            params![group_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(GroupAuthTxError::Io)?
        .unwrap_or(0);
    Ok(version as u64)
}

/// Transactional write of membership/roles + encrypted group state.
///
/// One transaction: optimistic version check, role mirror upsert, encrypted
/// state upsert (bumping `version`), commit.  On any error the transaction
/// rolls back and the group's persisted state is untouched.
///
/// Returns the new version on success.
pub fn save_group_state_and_roles(
    conn: &mut Connection,
    group_id: &GroupId,
    state: &GroupEncryptionState,
    roles: &HashMap<PeerId, MemberRole>,
    self_id: Option<PeerId>,
    expected_version: u64,
) -> Result<u64, GroupAuthTxError> {
    save_group_state_and_roles_inner(
        conn,
        group_id,
        state,
        roles,
        self_id,
        expected_version,
        None,
    )
}

/// Test-only fault-injecting variant of [`save_group_state_and_roles`].
pub fn save_group_state_and_roles_with_fault(
    conn: &mut Connection,
    group_id: &GroupId,
    state: &GroupEncryptionState,
    roles: &HashMap<PeerId, MemberRole>,
    self_id: Option<PeerId>,
    expected_version: u64,
    fault: GroupAuthFault,
) -> Result<u64, GroupAuthTxError> {
    save_group_state_and_roles_inner(
        conn,
        group_id,
        state,
        roles,
        self_id,
        expected_version,
        Some(fault),
    )
}

fn save_group_state_and_roles_inner(
    conn: &mut Connection,
    group_id: &GroupId,
    state: &GroupEncryptionState,
    roles: &HashMap<PeerId, MemberRole>,
    self_id: Option<PeerId>,
    expected_version: u64,
    fault: Option<GroupAuthFault>,
) -> Result<u64, GroupAuthTxError> {
    // Ensure the version column exists (idempotent; older DBs lack it).
    ensure_version_column(conn).map_err(GroupAuthTxError::Io)?;

    // BEGIN — one transaction owns the whole mutation.
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(GroupAuthTxError::Io)?;

    // Optimistic concurrency: the caller must be committing from the same
    // version it loaded.  A concurrent epoch operation that already bumped
    // the version makes this write fail instead of silently forking state.
    let current: i64 = tx
        .query_row(
            "SELECT version FROM group_encryption_state WHERE group_id = ?1",
            params![group_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(GroupAuthTxError::Io)?
        .unwrap_or(0);
    if current as u64 != expected_version {
        return Err(GroupAuthTxError::VersionConflict {
            expected: expected_version,
            current: current as u64,
        });
    }
    let new_version = current + 1;

    // 1. Membership/roles row (authorization state).
    tx.execute_batch(ROLES_TABLE_SQL)
        .map_err(GroupAuthTxError::Io)?;
    let roles_blob = postcard::to_stdvec(roles)
        .map_err(|e| GroupAuthTxError::Io(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;
    tx.execute(
        "INSERT OR REPLACE INTO group_encryption_roles (group_id, roles, self_id) VALUES (?1, ?2, ?3)",
        params![
            group_id.as_bytes().as_slice(),
            roles_blob,
            self_id.map(|p| p.0.as_bytes().to_vec())
        ],
    )
    .map_err(GroupAuthTxError::Io)?;

    if fault == Some(GroupAuthFault::AfterMembershipWrite) {
        return Err(GroupAuthTxError::Io(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some("injected fault: after membership write".into()),
        )));
    }

    // 2. Encrypted group state (crypto state), version bumped.
    let blob = encode_envelope(state)
        .map_err(|e| GroupAuthTxError::Io(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;
    let now = now_ms();
    tx.execute(
        "INSERT INTO group_encryption_state (group_id, state, updated_at, version) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(group_id) DO UPDATE SET state=excluded.state, updated_at=excluded.updated_at, version=excluded.version",
        params![group_id.as_bytes().as_slice(), blob, now, new_version],
    )
    .map_err(GroupAuthTxError::Io)?;

    if fault == Some(GroupAuthFault::AfterCryptoStateWrite) {
        return Err(GroupAuthTxError::Io(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some("injected fault: after crypto-state write".into()),
        )));
    }

    // COMMIT — only now is the new state visible.
    tx.commit().map_err(GroupAuthTxError::Io)?;
    Ok(new_version as u64)
}

// ── Role mirror persistence ─────────────────────────────────────────────
//
// The Kith-style role mirror (`group_roles`) and the local identity
// (`self_ids`) live outside the p2panda GroupState blob, so they are
// persisted in a companion table.  The table is created lazily on first use
// (following the repo's sqlite-kv-store pattern) so no storage.rs migration
// is required.

const ROLES_TABLE_SQL: &str = "\
CREATE TABLE IF NOT EXISTS group_encryption_roles (
    group_id BLOB PRIMARY KEY,
    roles    BLOB NOT NULL,
    self_id  BLOB
);";

/// Save (insert or update) the role mirror for a group.
pub fn save_group_roles(
    conn: &Connection,
    group_id: &GroupId,
    roles: &std::collections::HashMap<PeerId, MemberRole>,
    self_id: Option<PeerId>,
) -> rusqlite::Result<()> {
    conn.execute_batch(ROLES_TABLE_SQL)?;
    let blob = postcard::to_stdvec(roles)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute(
        "INSERT OR REPLACE INTO group_encryption_roles (group_id, roles, self_id) VALUES (?1, ?2, ?3)",
        params![
            group_id.as_bytes().as_slice(),
            blob,
            self_id.map(|p| p.0.as_bytes().to_vec())
        ],
    )?;
    Ok(())
}

/// Load the role mirror for a group, if one exists.
///
/// A row that exists but cannot be decoded (corrupt blob, wrong-length or
/// invalid `self_id`) fails closed with [`GroupStateLoadError::Corrupt`];
/// it is never reported as missing.
pub fn load_group_roles(
    conn: &Connection,
    group_id: &GroupId,
) -> Result<
    Option<(
        std::collections::HashMap<PeerId, MemberRole>,
        Option<PeerId>,
    )>,
    GroupStateLoadError,
> {
    conn.execute_batch(ROLES_TABLE_SQL)
        .map_err(GroupStateLoadError::Io)?;
    let mut stmt = conn
        .prepare("SELECT roles, self_id FROM group_encryption_roles WHERE group_id = ?1")
        .map_err(GroupStateLoadError::Io)?;
    let mut rows = stmt
        .query(params![group_id.as_bytes().as_slice()])
        .map_err(GroupStateLoadError::Io)?;
    match rows.next().map_err(GroupStateLoadError::Io)? {
        Some(row) => {
            let blob: Vec<u8> = row.get(0).map_err(GroupStateLoadError::Io)?;
            let self_id_bytes: Option<Vec<u8>> = row.get(1).map_err(GroupStateLoadError::Io)?;
            let roles: std::collections::HashMap<PeerId, MemberRole> = postcard::from_bytes(&blob)
                .map_err(|e| GroupStateLoadError::Corrupt {
                    group_id: *group_id,
                    reason: format!("role mirror decode failed: {e}"),
                })?;
            let self_id = match self_id_bytes {
                None => None,
                Some(b) => {
                    let arr: [u8; 32] = b.try_into().map_err(|_| GroupStateLoadError::Corrupt {
                        group_id: *group_id,
                        reason: "stored self_id is not exactly 32 bytes".to_string(),
                    })?;
                    let vk = p2panda_core::VerifyingKey::from_bytes(&arr).map_err(|e| {
                        GroupStateLoadError::Corrupt {
                            group_id: *group_id,
                            reason: format!("stored self_id is not a valid ed25519 key: {e}"),
                        }
                    })?;
                    Some(PeerId(vk))
                }
            };
            Ok(Some((roles, self_id)))
        }
        None => Ok(None),
    }
}

/// Delete the role mirror for a group.
pub fn delete_group_roles(conn: &Connection, group_id: &GroupId) -> rusqlite::Result<()> {
    conn.execute_batch(ROLES_TABLE_SQL)?;
    conn.execute(
        "DELETE FROM group_encryption_roles WHERE group_id = ?1",
        params![group_id.as_bytes().as_slice()],
    )?;
    Ok(())
}

/// Load the encryption state for a group.
///
/// Fails closed on every condition except a genuinely missing record:
///
/// - No row → [`GroupStateLoadError::Missing`] (the only condition that
///   permits fresh initialization).
/// - Row exists but cannot be decoded or validated →
///   [`GroupStateLoadError::Corrupt`]. The raw record is preserved — it is
///   never overwritten or deleted automatically.
/// - Row decodes to an unsupported format version →
///   [`GroupStateLoadError::UnsupportedVersion`] (route through migration).
pub fn load_group_state(
    conn: &Connection,
    group_id: &GroupId,
) -> Result<GroupEncryptionState, GroupStateLoadError> {
    let mut stmt = conn
        .prepare("SELECT state FROM group_encryption_state WHERE group_id = ?1")
        .map_err(GroupStateLoadError::Io)?;

    let mut rows = stmt
        .query(params![group_id.as_bytes().as_slice()])
        .map_err(GroupStateLoadError::Io)?;

    match rows.next().map_err(GroupStateLoadError::Io)? {
        Some(row) => {
            let blob: Vec<u8> = row.get(0).map_err(GroupStateLoadError::Io)?;
            decode_group_state_blob(group_id, &blob)
        }
        None => Err(GroupStateLoadError::Missing),
    }
}

/// Decode a stored state blob, separating corruption from unsupported
/// versions and validating decoded invariants.
///
/// Format detection order matters:
///
/// 1. Legacy **v0** (raw, unversioned [`GroupEncryptionState`]) — decodes
///    successfully → [`GroupStateLoadError::UnsupportedVersion`] with
///    `version: 0`, so the record is routed through migration instead of
///    being treated as corruption or silently re-saved.
/// 2. Current **v1** envelope ([`StoredGroupStateV1`]) — version matches →
///    validate invariants → return the state; version mismatch →
///    [`GroupStateLoadError::UnsupportedVersion`].
/// 3. Neither decodes → [`GroupStateLoadError::Corrupt`].
fn decode_group_state_blob(
    group_id: &GroupId,
    blob: &[u8],
) -> Result<GroupEncryptionState, GroupStateLoadError> {
    // Legacy v0: the raw (unversioned) GroupState written before versioning.
    // A blob that decodes here is not corrupt — it needs migration.  This
    // check must run BEFORE the checksum verification because a legacy blob
    // has no checksum.
    if let Ok(state) = postcard::from_bytes::<GroupEncryptionState>(blob) {
        let _ = state;
        return Err(GroupStateLoadError::UnsupportedVersion {
            group_id: *group_id,
            version: 0,
        });
    }

    // Current v1: versioned envelope. Verify the raw-record integrity
    // checksum first so ANY bit flip (including one inside a raw key byte
    // that postcard tolerates) is detected as corruption, not accepted.
    if blob.len() < 32 {
        return Err(GroupStateLoadError::Corrupt {
            group_id: *group_id,
            reason: "stored state blob is too short to contain a checksum".to_string(),
        });
    }
    let (body, stored_checksum) = blob.split_at(blob.len() - 32);
    let expected_checksum = *blake3::hash(body).as_bytes();
    if expected_checksum != stored_checksum {
        return Err(GroupStateLoadError::Corrupt {
            group_id: *group_id,
            reason: "integrity checksum mismatch (stored record corrupted)".to_string(),
        });
    }

    let envelope: StoredGroupStateV1 =
        postcard::from_bytes(blob).map_err(|e| GroupStateLoadError::Corrupt {
            group_id: *group_id,
            reason: format!("postcard decode failed: {e}"),
        })?;
    if envelope.version != GROUP_STATE_STORAGE_VERSION {
        return Err(GroupStateLoadError::UnsupportedVersion {
            group_id: *group_id,
            version: envelope.version,
        });
    }
    validate_decoded_state(group_id, &envelope.state)?;
    Ok(envelope.state)
}

/// Validate decoded invariants after deserialization.
///
/// The p2panda [`GroupState`] fields are crate-private, so the strongest
/// check available at this layer is exercising the DGM member view: a decoded
/// but internally inconsistent membership state fails here and is reported as
/// corruption rather than silently accepted.  Additional binding checks
/// (local identity ∈ members, role-mirror ⊆ members) run in
/// [`super::encryption_state::EncryptionState::load_group_state_from_db`]
/// where the companion role mirror is available.
fn validate_decoded_state(
    group_id: &GroupId,
    state: &GroupEncryptionState,
) -> Result<(), GroupStateLoadError> {
    MessageGroup::members(state).map_err(|e| GroupStateLoadError::Corrupt {
        group_id: *group_id,
        reason: format!("decoded state failed membership validation: {e}"),
    })?;
    Ok(())
}

/// Delete the encryption state for a group.
///
/// Called when a room is deleted or encryption is disabled.  This is a
/// deliberate application-level deletion — it does NOT quarantine.  Corrupt
/// records that need recovery must go through [`quarantine_group_state`]
/// / [`reset_group_state`] instead so the raw bytes are preserved.
pub fn delete_group_state(conn: &Connection, group_id: &GroupId) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM group_encryption_state WHERE group_id = ?1",
        params![group_id.as_bytes().as_slice()],
    )?;
    Ok(())
}

// ── Quarantine / recovery ─────────────────────────────────────────────
//
// Corrupt or unsupported state records are NEVER overwritten automatically.
// Before any user-approved reset the raw record is moved into a quarantine
// table so it can be inspected or recovered.

const STATE_QUARANTINE_TABLE_SQL: &str = "\
CREATE TABLE IF NOT EXISTS group_encryption_state_quarantine (
    group_id   BLOB PRIMARY KEY,
    state      BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    quarantined_at INTEGER NOT NULL
);";

const ROLES_QUARANTINE_TABLE_SQL: &str = "\
CREATE TABLE IF NOT EXISTS group_encryption_roles_quarantine (
    group_id BLOB PRIMARY KEY,
    roles    BLOB NOT NULL,
    self_id  BLOB,
    quarantined_at INTEGER NOT NULL
);";

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Move the raw state record for `group_id` into the quarantine table,
/// preserving its bytes for diagnostics/recovery, then remove it from the
/// live table.
///
/// This is the only path that discards a corrupt/unsupported live record and
/// it must only be invoked from an explicit user-approved recovery flow.
/// Returns `Ok(true)` if a record was moved, `Ok(false)` if none existed.
pub fn quarantine_group_state(
    conn: &Connection,
    group_id: &GroupId,
) -> Result<bool, GroupStateLoadError> {
    conn.execute_batch(STATE_QUARANTINE_TABLE_SQL)
        .map_err(GroupStateLoadError::Io)?;
    let moved = conn
        .execute(
            "INSERT OR REPLACE INTO group_encryption_state_quarantine \
             (group_id, state, updated_at, quarantined_at) \
             SELECT group_id, state, updated_at, ?1 \
             FROM group_encryption_state WHERE group_id = ?2",
            params![now_ms(), group_id.as_bytes().as_slice()],
        )
        .map_err(GroupStateLoadError::Io)?;
    if moved == 0 {
        return Ok(false);
    }
    conn.execute(
        "DELETE FROM group_encryption_state WHERE group_id = ?1",
        params![group_id.as_bytes().as_slice()],
    )
    .map_err(GroupStateLoadError::Io)?;
    Ok(true)
}

/// Move the raw role-mirror record for `group_id` into the quarantine table.
pub fn quarantine_group_roles(
    conn: &Connection,
    group_id: &GroupId,
) -> Result<bool, GroupStateLoadError> {
    conn.execute_batch(ROLES_QUARANTINE_TABLE_SQL)
        .map_err(GroupStateLoadError::Io)?;
    let moved = conn
        .execute(
            "INSERT OR REPLACE INTO group_encryption_roles_quarantine \
             (group_id, roles, self_id, quarantined_at) \
             SELECT group_id, roles, self_id, ?1 \
             FROM group_encryption_roles WHERE group_id = ?2",
            params![now_ms(), group_id.as_bytes().as_slice()],
        )
        .map_err(GroupStateLoadError::Io)?;
    if moved == 0 {
        return Ok(false);
    }
    conn.execute(
        "DELETE FROM group_encryption_roles WHERE group_id = ?1",
        params![group_id.as_bytes().as_slice()],
    )
    .map_err(GroupStateLoadError::Io)?;
    Ok(true)
}

/// Explicit, user-approved reset: quarantine any existing state and role
/// mirror records for `group_id`, preserving their raw bytes, then remove
/// them from the live tables.
///
/// After this returns, [`load_group_state`] reports
/// [`GroupStateLoadError::Missing`] for the group and fresh initialization
/// becomes permissible.  Only call this from an explicit recovery path —
/// never automatically in response to a decode failure.
pub fn reset_group_state(conn: &Connection, group_id: &GroupId) -> Result<(), GroupStateLoadError> {
    let _ = quarantine_group_state(conn, group_id)?;
    let _ = quarantine_group_roles(conn, group_id)?;
    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a minimal GroupEncryptionState for testing.
    fn make_test_state() -> (GroupId, GroupEncryptionState) {
        use crate::group_encryption::encryption_state::EncryptionState;
        use crate::group_encryption::types::PeerId;
        use p2panda_encryption::crypto::x25519::SecretKey as XSecretKey;
        use p2panda_encryption::crypto::xeddsa::xeddsa_sign;
        use p2panda_encryption::crypto::Rng;
        use p2panda_encryption::key_bundle::{Lifetime, OneTimeKeyBundle, OneTimePreKey, PreKey};

        fn make_bundle(rng: &Rng) -> OneTimeKeyBundle {
            let secret_key = XSecretKey::from_rng(rng).unwrap();
            let identity_key = secret_key.verifying_key().unwrap();
            let signed_prekey_secret = XSecretKey::from_rng(rng).unwrap();
            let signed_prekey = PreKey::new(
                signed_prekey_secret.verifying_key().unwrap(),
                Lifetime::default(),
            );
            let prekey_signature = xeddsa_sign(signed_prekey.as_bytes(), &secret_key, rng).unwrap();
            let onetime_prekey_secret = XSecretKey::from_rng(rng).unwrap();
            let onetime_prekey =
                OneTimePreKey::new(onetime_prekey_secret.verifying_key().unwrap(), 1);
            OneTimeKeyBundle::new(
                identity_key,
                signed_prekey,
                prekey_signature,
                Some(onetime_prekey),
            )
        }

        let group_id = GroupId::generate();
        let rng = Rng::default();
        let mut enc_state = EncryptionState::new_with_rng(rng).unwrap();
        let sk = iroh::SecretKey::generate();
        let my_id = PeerId::from(sk.public());

        // Register the peer's identity key in the registry so
        // MessageGroup::create can look it up.
        let bundle = make_bundle(&enc_state.rng);
        enc_state
            .registry
            .insert_identity(&my_id, &bundle)
            .expect("insert identity");

        let _envelope = enc_state
            .create_group(group_id, my_id, vec![])
            .expect("create_group");

        let state = enc_state
            .groups
            .remove(&group_id)
            .expect("group state after create");

        (group_id, state)
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();

        let (group_id, state) = make_test_state();

        // Save
        save_group_state(&conn, &group_id, &state).unwrap();

        // Load — returns the state directly (no Option wrapper).
        let loaded = load_group_state(&conn, &group_id).unwrap();

        // Verify: the loaded state is valid and can be re-saved with no error.
        save_group_state(&conn, &group_id, &loaded).unwrap();
        // The re-loaded state must still pass integrity + membership checks.
        let reloaded = load_group_state(&conn, &group_id).unwrap();
        let members = MessageGroup::members(&reloaded).unwrap();
        assert!(!members.is_empty(), "round-tripped state has members");
    }

    /// Missing state for a genuinely new group → initialization allowed.
    #[test]
    fn test_load_missing_permits_fresh_init() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();

        let group_id = GroupId::generate();
        match load_group_state(&conn, &group_id) {
            Err(GroupStateLoadError::Missing) => {}
            other => {
                panic!("expected GroupStateLoadError::Missing for a new group, got: {other:?}")
            }
        }
    }

    #[test]
    fn test_delete_removes_state() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();

        let (group_id, state) = make_test_state();

        save_group_state(&conn, &group_id, &state).unwrap();
        assert!(
            load_group_state(&conn, &group_id).is_ok(),
            "state should exist after save"
        );

        delete_group_state(&conn, &group_id).unwrap();
        assert!(
            matches!(
                load_group_state(&conn, &group_id),
                Err(GroupStateLoadError::Missing)
            ),
            "state should be gone after delete"
        );
    }

    #[test]
    fn test_overwrite_updates_state() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();

        let (group_id, state) = make_test_state();

        // Save twice
        save_group_state(&conn, &group_id, &state).unwrap();
        save_group_state(&conn, &group_id, &state).unwrap();

        // Load — should succeed
        let loaded = load_group_state(&conn, &group_id).unwrap();

        // Verify updated_at was refreshed: save again and check updated_at
        save_group_state(&conn, &group_id, &loaded).unwrap();

        let mut stmt = conn
            .prepare("SELECT updated_at FROM group_encryption_state WHERE group_id = ?1")
            .unwrap();
        let updated_at: i64 = stmt
            .query_row(params![group_id.as_bytes().as_slice()], |row| row.get(0))
            .unwrap();

        assert!(updated_at > 0, "updated_at should be a positive timestamp");
    }

    // ── Fail-closed regression tests (BORU-AUDIT-04) ────────────────────

    /// Helper: flip one byte in the middle of the stored state blob and write
    /// it back, simulating disk/DB corruption.
    fn corrupt_stored_blob(conn: &Connection, group_id: &GroupId) -> Vec<u8> {
        let mut stmt = conn
            .prepare("SELECT state FROM group_encryption_state WHERE group_id = ?1")
            .unwrap();
        let mut blob: Vec<u8> = stmt
            .query_row(params![group_id.as_bytes().as_slice()], |r| r.get(0))
            .unwrap();
        assert!(blob.len() > 32, "stored blob should be non-trivial");
        let mid = blob.len() / 2;
        blob[mid] ^= 0xFF;
        conn.execute(
            "UPDATE group_encryption_state SET state = ?1 WHERE group_id = ?2",
            params![blob.clone(), group_id.as_bytes().as_slice()],
        )
        .unwrap();
        blob
    }

    /// Existing state with one corrupted byte → load fails closed and no new
    /// state is persisted.
    #[test]
    fn test_corrupt_state_fails_closed_no_overwrite() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();

        let (group_id, state) = make_test_state();
        save_group_state(&conn, &group_id, &state).unwrap();

        let corrupted = corrupt_stored_blob(&conn, &group_id);

        // Load must fail closed with Corrupt — NOT Ok(None)/Missing.
        match load_group_state(&conn, &group_id) {
            Err(GroupStateLoadError::Corrupt { .. }) => {}
            other => panic!("expected Corrupt for corrupted blob, got: {other:?}"),
        }

        // The raw record is preserved: the stored blob is still the corrupted
        // bytes, not a freshly-regenerated state.
        let mut stmt = conn
            .prepare("SELECT state FROM group_encryption_state WHERE group_id = ?1")
            .unwrap();
        let stored: Vec<u8> = stmt
            .query_row(params![group_id.as_bytes().as_slice()], |r| r.get(0))
            .unwrap();
        assert_eq!(
            stored, corrupted,
            "corrupt record must NOT be overwritten automatically"
        );
    }

    /// Corrupt state survives a simulated restart (fresh load path) without
    /// being overwritten.
    #[test]
    fn test_corrupt_state_survives_reload() {
        let (group_id, state) = make_test_state();
        let db_path = std::env::temp_dir().join(format!(
            "boru_group_state_corrupt_{}_{}.db",
            std::process::id(),
            hex::encode(group_id.as_bytes())
        ));
        let _ = std::fs::remove_file(&db_path);

        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();

        save_group_state(&conn, &group_id, &state).unwrap();
        let corrupted = corrupt_stored_blob(&conn, &group_id);
        drop(conn);

        // "Restart": open the same on-disk DB with a fresh connection.
        let conn2 = Connection::open(&db_path).unwrap();
        match load_group_state(&conn2, &group_id) {
            Err(GroupStateLoadError::Corrupt { .. }) => {}
            other => panic!("expected Corrupt after restart, got: {other:?}"),
        }
        let stored: Vec<u8> = {
            let mut stmt = conn2
                .prepare("SELECT state FROM group_encryption_state WHERE group_id = ?1")
                .unwrap();
            stmt.query_row(params![group_id.as_bytes().as_slice()], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(
            stored, corrupted,
            "corrupt record must survive restart without being overwritten"
        );
        drop(conn2);
        let _ = std::fs::remove_file(&db_path);
    }

    /// Unsupported (future) version → migration/version error, not "missing".
    #[test]
    fn test_unsupported_version_is_migration_error() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();

        let (group_id, state) = make_test_state();
        save_group_state(&conn, &group_id, &state).unwrap();

        // Overwrite with a future-version envelope (checksum still valid).
        let envelope = StoredGroupStateV1 {
            version: 99,
            state,
            checksum: [0u8; 32],
        };
        let mut blob = postcard::to_stdvec(&envelope).unwrap();
        let body_len = blob.len() - 32;
        let checksum = *blake3::hash(&blob[..body_len]).as_bytes();
        blob[body_len..].copy_from_slice(&checksum);
        conn.execute(
            "UPDATE group_encryption_state SET state = ?1 WHERE group_id = ?2",
            params![blob, group_id.as_bytes().as_slice()],
        )
        .unwrap();

        match load_group_state(&conn, &group_id) {
            Err(GroupStateLoadError::UnsupportedVersion { version, .. }) => {
                assert_eq!(version, 99, "should report the stored version");
            }
            other => panic!("expected UnsupportedVersion for future format, got: {other:?}"),
        }
    }

    /// Legacy (v0, unversioned) blob → version error routed through
    /// migration, not corruption and not missing.
    #[test]
    fn test_legacy_v0_blob_is_migration_error() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();

        let (group_id, state) = make_test_state();
        save_group_state(&conn, &group_id, &state).unwrap();

        // Overwrite with a raw (unversioned) GroupState blob — the pre-
        // versioning format.
        let raw = postcard::to_stdvec(&state).unwrap();
        conn.execute(
            "UPDATE group_encryption_state SET state = ?1 WHERE group_id = ?2",
            params![raw, group_id.as_bytes().as_slice()],
        )
        .unwrap();

        match load_group_state(&conn, &group_id) {
            Err(GroupStateLoadError::UnsupportedVersion { version, .. }) => {
                assert_eq!(version, 0, "legacy raw blob is version 0");
            }
            other => panic!("expected UnsupportedVersion(v0) for legacy blob, got: {other:?}"),
        }
    }

    /// Quarantine preserves the raw corrupt record for diagnostics/recovery
    /// and only then makes the group load as Missing.
    #[test]
    fn test_quarantine_preserves_raw_record() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();

        let (group_id, state) = make_test_state();
        save_group_state(&conn, &group_id, &state).unwrap();
        let corrupted = corrupt_stored_blob(&conn, &group_id);

        // User-approved recovery: quarantine the corrupt record.
        let moved = quarantine_group_state(&conn, &group_id).unwrap();
        assert!(moved, "quarantine should move the corrupt record");

        // Raw bytes preserved in the quarantine table.
        let mut stmt = conn
            .prepare("SELECT state FROM group_encryption_state_quarantine WHERE group_id = ?1")
            .unwrap();
        let quarantined: Vec<u8> = stmt
            .query_row(params![group_id.as_bytes().as_slice()], |r| r.get(0))
            .unwrap();
        assert_eq!(quarantined, corrupted, "quarantine must preserve raw bytes");

        // Live row gone → fresh initialization becomes permissible.
        assert!(
            matches!(
                load_group_state(&conn, &group_id),
                Err(GroupStateLoadError::Missing)
            ),
            "after quarantine the group loads as Missing (fresh init allowed)"
        );

        // Quarantining again is a no-op.
        let moved_again = quarantine_group_state(&conn, &group_id).unwrap();
        assert!(!moved_again, "second quarantine should move nothing");
    }

    /// reset_group_state quarantines state AND role mirror, and the group
    /// loads as Missing afterwards.
    #[test]
    fn test_reset_allows_fresh_init_after_user_action() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();

        let (group_id, state) = make_test_state();
        save_group_state(&conn, &group_id, &state).unwrap();
        let (roles, self_id) = make_roles();
        save_group_roles(&conn, &group_id, &roles, Some(self_id)).unwrap();

        reset_group_state(&conn, &group_id).unwrap();

        assert!(
            matches!(
                load_group_state(&conn, &group_id),
                Err(GroupStateLoadError::Missing)
            ),
            "state must be missing after explicit reset"
        );
        assert!(
            load_group_roles(&conn, &group_id).unwrap().is_none(),
            "role mirror must be gone after explicit reset"
        );
        // Quarantine tables received the raw records.
        let state_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM group_encryption_state_quarantine",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let roles_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM group_encryption_roles_quarantine",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state_count, 1, "state quarantine should hold the record");
        assert_eq!(roles_count, 1, "roles quarantine should hold the record");
    }

    // ── Role mirror persistence tests ──────────────────────────────────

    /// Helper: build a roles map + self id for persistence tests.
    fn make_roles() -> (std::collections::HashMap<PeerId, MemberRole>, PeerId) {
        let owner = PeerId::from(iroh::SecretKey::generate().public());
        let member = PeerId::from(iroh::SecretKey::generate().public());
        let mut roles = std::collections::HashMap::new();
        roles.insert(owner, MemberRole::Admin);
        roles.insert(member, MemberRole::Reader);
        (roles, owner)
    }

    #[test]
    fn test_group_roles_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        let group_id = GroupId::generate();
        let (roles, self_id) = make_roles();

        // Lazy table creation happens inside save_group_roles — no migration.
        save_group_roles(&conn, &group_id, &roles, Some(self_id)).unwrap();

        let loaded = load_group_roles(&conn, &group_id)
            .unwrap()
            .expect("roles should exist");
        assert_eq!(loaded.0, roles, "roles round-trip");
        assert_eq!(loaded.1, Some(self_id), "self id round-trips");
    }

    #[test]
    fn test_group_roles_delete() {
        let conn = Connection::open_in_memory().unwrap();
        let group_id = GroupId::generate();
        let (roles, self_id) = make_roles();

        save_group_roles(&conn, &group_id, &roles, Some(self_id)).unwrap();
        assert!(
            load_group_roles(&conn, &group_id).unwrap().is_some(),
            "roles exist after save"
        );

        delete_group_roles(&conn, &group_id).unwrap();
        assert!(
            load_group_roles(&conn, &group_id).unwrap().is_none(),
            "roles gone after delete"
        );
    }

    #[test]
    fn test_group_roles_missing_returns_none() {
        let conn = Connection::open_in_memory().unwrap();
        let group_id = GroupId::generate();
        assert!(
            load_group_roles(&conn, &group_id).unwrap().is_none(),
            "no roles for unknown group"
        );
    }

    // ── Transactional group-auth tests (BORU-AUDIT-09) ────────────────

    /// Helper: create the state table exactly as migration v13 does (no
    /// version column) and return a mutable connection for transactional
    /// writes.
    fn make_tx_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        conn
    }

    /// `save_group_state_and_roles` commits BOTH the role mirror and the
    /// encrypted state in one transaction and bumps the version.
    #[test]
    fn test_tx_commits_state_and_roles_and_bumps_version() {
        let mut conn = make_tx_conn();
        let (group_id, state) = make_test_state();
        let (roles, self_id) = make_roles();

        let v1 = save_group_state_and_roles(&mut conn, &group_id, &state, &roles, Some(self_id), 0)
            .expect("first transactional write");
        assert_eq!(v1, 1, "fresh group commits at version 1");

        let v2 =
            save_group_state_and_roles(&mut conn, &group_id, &state, &roles, Some(self_id), v1)
                .expect("second transactional write");
        assert_eq!(v2, 2, "second write bumps to version 2");

        assert_eq!(load_group_version(&conn, &group_id).unwrap(), 2);
        let loaded_state = load_group_state(&conn, &group_id).expect("state persisted");
        assert!(
            MessageGroup::members(&loaded_state).is_ok(),
            "persisted state still decodes"
        );
        let loaded_roles = load_group_roles(&conn, &group_id)
            .unwrap()
            .expect("roles persisted");
        assert_eq!(loaded_roles.0, roles, "role mirror persisted");
        assert_eq!(loaded_roles.1, Some(self_id), "self id persisted");
    }

    /// Injecting a failure after the membership write but before the
    /// crypto-state write rolls the transaction back COMPLETELY: neither the
    /// role mirror nor the encrypted state is persisted.
    #[test]
    fn test_tx_fault_after_membership_write_rolls_back_completely() {
        let mut conn = make_tx_conn();
        let (group_id, state) = make_test_state();
        let (roles, self_id) = make_roles();

        let err = save_group_state_and_roles_with_fault(
            &mut conn,
            &group_id,
            &state,
            &roles,
            Some(self_id),
            0,
            GroupAuthFault::AfterMembershipWrite,
        )
        .expect_err("injected fault must fail the transaction");

        assert!(
            matches!(err, GroupAuthTxError::Io(_)),
            "fault surfaces as Io error, got: {err:?}"
        );

        // No partial state: the group still loads as Missing and roles are
        // absent — the membership write was rolled back.
        assert!(
            matches!(
                load_group_state(&conn, &group_id),
                Err(GroupStateLoadError::Missing)
            ),
            "crypto state must NOT exist after rollback"
        );
        assert!(
            load_group_roles(&conn, &group_id).unwrap().is_none(),
            "role mirror must NOT exist after rollback"
        );
    }

    /// Injecting a failure after the crypto-state write but before commit
    /// also rolls the transaction back completely.
    #[test]
    fn test_tx_fault_after_crypto_state_write_rolls_back_completely() {
        let mut conn = make_tx_conn();
        let (group_id, state) = make_test_state();
        let (roles, self_id) = make_roles();

        let err = save_group_state_and_roles_with_fault(
            &mut conn,
            &group_id,
            &state,
            &roles,
            Some(self_id),
            0,
            GroupAuthFault::AfterCryptoStateWrite,
        )
        .expect_err("injected fault must fail the transaction");

        assert!(
            matches!(err, GroupAuthTxError::Io(_)),
            "fault surfaces as Io error, got: {err:?}"
        );

        assert!(
            matches!(
                load_group_state(&conn, &group_id),
                Err(GroupStateLoadError::Missing)
            ),
            "crypto state must NOT exist after rollback"
        );
        assert!(
            load_group_roles(&conn, &group_id).unwrap().is_none(),
            "role mirror must NOT exist after rollback"
        );
    }

    /// A stale expected version (concurrent mutation already committed) is
    /// rejected: no write happens and the stored version is untouched.
    #[test]
    fn test_tx_version_conflict_rejects_stale_writer() {
        let mut conn = make_tx_conn();
        let (group_id, state) = make_test_state();
        let (roles, self_id) = make_roles();

        save_group_state_and_roles(&mut conn, &group_id, &state, &roles, Some(self_id), 0)
            .expect("first write at v0");

        // Second writer still believes the base version is 0 → conflict.
        let err =
            save_group_state_and_roles(&mut conn, &group_id, &state, &roles, Some(self_id), 0)
                .expect_err("stale expected version must conflict");
        match err {
            GroupAuthTxError::VersionConflict {
                expected: 0,
                current: 1,
            } => {}
            other => panic!("expected VersionConflict{{expected:0,current:1}}, got: {other:?}"),
        }

        assert_eq!(
            load_group_version(&conn, &group_id).unwrap(),
            1,
            "stored version must remain at the committed value"
        );
    }

    /// A failed transaction preserves the previously committed state: the
    /// rollback leaves the old state + roles fully intact and loadable.
    #[test]
    fn test_tx_fault_preserves_prior_committed_state() {
        let mut conn = make_tx_conn();
        let (group_id, state) = make_test_state();
        let (roles, self_id) = make_roles();

        // Commit a base version.
        save_group_state_and_roles(&mut conn, &group_id, &state, &roles, Some(self_id), 0)
            .expect("base write");

        // Attempt a mutation that fails after the membership write.
        let (roles2, self_id2) = make_roles();
        let err = save_group_state_and_roles_with_fault(
            &mut conn,
            &group_id,
            &state,
            &roles2,
            Some(self_id2),
            1,
            GroupAuthFault::AfterMembershipWrite,
        )
        .expect_err("injected fault must fail");
        assert!(matches!(err, GroupAuthTxError::Io(_)));

        // The prior committed state is still fully intact.
        assert_eq!(load_group_version(&conn, &group_id).unwrap(), 1);
        let loaded_state = load_group_state(&conn, &group_id).expect("prior state preserved");
        assert!(
            MessageGroup::members(&loaded_state).is_ok(),
            "prior crypto state still decodes"
        );
        let loaded_roles = load_group_roles(&conn, &group_id)
            .unwrap()
            .expect("prior roles preserved");
        assert_eq!(loaded_roles.0, roles, "prior role mirror preserved");
        assert_eq!(loaded_roles.1, Some(self_id), "prior self id preserved");
    }
}
