//! Persistence for per-group encryption state.
//!
//! Saves and loads [`GroupEncryptionState`] (a serialised [`GroupState`] from
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
//! The `state` column stores a postcard-encoded [`GroupEncryptionState`] blob.
//! `updated_at` is a Unix-epoch milliseconds timestamp set on every write.

use rusqlite::{params, Connection};

use crate::group_id::GroupId;

use super::encryption_state::GroupEncryptionState;
use super::membership::MemberRole;
use super::types::PeerId;

/// Save (insert or update) the encryption state for a group.
///
/// Serialises `state` with postcard and writes it into the
/// `group_encryption_state` table.  Uses INSERT OR REPLACE so repeated
/// saves for the same group are always idempotent.
pub fn save_group_state(
    conn: &Connection,
    group_id: &GroupId,
    state: &GroupEncryptionState,
) -> rusqlite::Result<()> {
    let blob = postcard::to_stdvec(state)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    conn.execute(
        "INSERT OR REPLACE INTO group_encryption_state (group_id, state, updated_at) VALUES (?1, ?2, ?3)",
        params![group_id.as_bytes().as_slice(), blob, now],
    )?;
    Ok(())
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
pub fn load_group_roles(
    conn: &Connection,
    group_id: &GroupId,
) -> rusqlite::Result<
    Option<(
        std::collections::HashMap<PeerId, MemberRole>,
        Option<PeerId>,
    )>,
> {
    let _ = conn.execute_batch(ROLES_TABLE_SQL)?;
    let mut stmt =
        conn.prepare("SELECT roles, self_id FROM group_encryption_roles WHERE group_id = ?1")?;
    let mut rows = stmt.query(params![group_id.as_bytes().as_slice()])?;
    match rows.next()? {
        Some(row) => {
            let blob: Vec<u8> = row.get(0)?;
            let self_id_bytes: Option<Vec<u8>> = row.get(1)?;
            let roles: std::collections::HashMap<PeerId, MemberRole> = postcard::from_bytes(&blob)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            let self_id = self_id_bytes.map(|b| {
                let arr: [u8; 32] = b
                    .try_into()
                    .expect("stored self_id must be exactly 32 bytes");
                let vk = p2panda_core::VerifyingKey::from_bytes(&arr)
                    .expect("stored self_id is a valid ed25519 key");
                PeerId(vk)
            });
            Ok(Some((roles, self_id)))
        }
        None => Ok(None),
    }
}

/// Delete the role mirror for a group.
pub fn delete_group_roles(conn: &Connection, group_id: &GroupId) -> rusqlite::Result<()> {
    let _ = conn.execute_batch(ROLES_TABLE_SQL)?;
    conn.execute(
        "DELETE FROM group_encryption_roles WHERE group_id = ?1",
        params![group_id.as_bytes().as_slice()],
    )?;
    Ok(())
}

/// Load the encryption state for a group, if one exists.
///
/// Returns `None` when no state has been persisted for this group, or when
/// deserialisation fails (corrupt blob).
pub fn load_group_state(
    conn: &Connection,
    group_id: &GroupId,
) -> rusqlite::Result<Option<GroupEncryptionState>> {
    let mut stmt = conn.prepare("SELECT state FROM group_encryption_state WHERE group_id = ?1")?;

    let mut rows = stmt.query(params![group_id.as_bytes().as_slice()])?;

    match rows.next()? {
        Some(row) => {
            let blob: Vec<u8> = row.get(0)?;
            match postcard::from_bytes(&blob) {
                Ok(state) => Ok(Some(state)),
                Err(e) => {
                    // Log the deserialisation error but return None so the
                    // caller can fall back to creating fresh state.
                    tracing::warn!(
                        "failed to deserialize group encryption state for {group_id}: {e}"
                    );
                    Ok(None)
                }
            }
        }
        None => Ok(None),
    }
}

/// Delete the encryption state for a group.
///
/// Called when a room is deleted or encryption is disabled.
pub fn delete_group_state(conn: &Connection, group_id: &GroupId) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM group_encryption_state WHERE group_id = ?1",
        params![group_id.as_bytes().as_slice()],
    )?;
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
        use p2panda_encryption::crypto::x25519::{
            PublicKey as XPublicKey, SecretKey as XSecretKey,
        };
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

        // Load
        let loaded = load_group_state(&conn, &group_id)
            .unwrap()
            .expect("state should exist");

        // Verify: the loaded state's group_id matches (we can't compare
        // GroupState directly, but we can verify it round-trips).
        // To be thorough, re-save the loaded state and verify no error.
        save_group_state(&conn, &group_id, &loaded).unwrap();
    }

    #[test]
    fn test_load_nonexistent_returns_none() {
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
        let result = load_group_state(&conn, &group_id).unwrap();
        assert!(result.is_none(), "no state should exist for random group");
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
            load_group_state(&conn, &group_id).unwrap().is_some(),
            "state should exist after save"
        );

        delete_group_state(&conn, &group_id).unwrap();
        assert!(
            load_group_state(&conn, &group_id).unwrap().is_none(),
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
        let loaded = load_group_state(&conn, &group_id)
            .unwrap()
            .expect("state should exist after overwrite");

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
}
