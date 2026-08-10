//! SQLite-backed identity and pre-key registries for p2panda group encryption.
//!
//! # Tables
//!
//! - `identity_registry` — maps `PeerId → OneTimeKeyBundle` (identity key + signed
//!   pre-key + optional one-time pre-key).  Looked up by [`IdentityRegistry`].
//! - `prekey_registry` — one-time pre-key bundles per peer.  Each row stores a
//!   single `OneTimeKeyBundle`; the `used` flag marks consumed bundles.
//!   Looked up by [`PreKeyRegistry`].
//!
//! # State
//!
//! [`RegistryState`](crate::group_encryption::registry::RegistryState) wraps a shared SQLite connection (`Arc<Mutex<Connection>>`)
//! so that both registry traits work from the same connection handle.  The
//! [`Registry`](crate::group_encryption::registry::Registry) unit struct carries the two trait implementations.
//!
//! [`IdentityRegistry`]: p2panda_encryption::traits::IdentityRegistry
//! [`PreKeyRegistry`]: p2panda_encryption::traits::PreKeyRegistry

use std::fmt;
use std::sync::{Arc, Mutex};

use p2panda_encryption::crypto::x25519::PublicKey;
use p2panda_encryption::key_bundle::OneTimeKeyBundle;
use p2panda_encryption::traits::{IdentityRegistry, KeyBundle, PreKeyRegistry};
use rusqlite::{params, Connection, OptionalExtension};
use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::types::PeerId;

// ── RegistryState ──────────────────────────────────────────────────────────

/// Shared SQLite-backed state for identity and pre-key registries.
///
/// Holds an `Arc<Mutex<Connection>>` so it can be cheaply cloned and used
/// concurrently from both [`IdentityRegistry`] and [`PreKeyRegistry`] trait
/// implementations.
#[derive(Clone)]
pub struct RegistryState {
    conn: Arc<Mutex<Connection>>,
}

impl fmt::Debug for RegistryState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegistryState").finish_non_exhaustive()
    }
}

impl RegistryState {
    /// Create a new registry state from an existing SQLite connection.
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Return a reference to the shared connection.
    pub fn conn(&self) -> &Arc<Mutex<Connection>> {
        &self.conn
    }

    /// Insert (or replace) a key bundle for `peer_id` in the identity registry.
    ///
    /// The bundle is serialized with postcard and stored in the
    /// `identity_registry` table.
    pub fn insert_identity(
        &self,
        peer_id: &PeerId,
        bundle: &OneTimeKeyBundle,
    ) -> Result<(), RegistryError> {
        let peer_bytes = peer_id.0.as_bytes().to_vec();
        let bundle_bytes = postcard::to_allocvec(bundle)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO identity_registry (peer_id, key_bundle) VALUES (?1, ?2)",
            params![peer_bytes, bundle_bytes],
        )?;
        Ok(())
    }

    /// Insert one-time pre-key bundles for `peer_id`.
    ///
    /// Each bundle is stored as a separate row with `used = 0`.
    pub fn insert_pre_keys(
        &self,
        peer_id: &PeerId,
        bundles: &[OneTimeKeyBundle],
    ) -> Result<(), RegistryError> {
        let peer_bytes = peer_id.0.as_bytes().to_vec();
        let conn = self.conn.lock().unwrap();
        for bundle in bundles {
            let bundle_bytes = postcard::to_allocvec(bundle)?;
            conn.execute(
                "INSERT INTO prekey_registry (peer_id, pre_key) VALUES (?1, ?2)",
                params![peer_bytes, bundle_bytes],
            )?;
        }
        Ok(())
    }
}

// ── Serialization support ─────────────────────────────────────────────────
//
// RegistryState's database connection handle is not serialisable.
// `Serialize` emits a unit value; `Deserialize` creates a fresh in-memory
// database with the required tables.  This means the identity/pre-key data
// is lost on deserialization — callers should re-populate the registry
// from the network or disk when needed.

impl Serialize for RegistryState {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_unit()
    }
}

impl<'de> Deserialize<'de> for RegistryState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RegistryStateVisitor;
        impl<'de> de::Visitor<'de> for RegistryStateVisitor {
            type Value = RegistryState;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a RegistryState with a fresh in-memory database")
            }

            fn visit_unit<E: de::Error>(self) -> Result<RegistryState, E> {
                // Create a fresh in-memory database with the required tables.
                let conn = Connection::open_in_memory().map_err(de::Error::custom)?;
                conn.execute_batch(
                    "
                    CREATE TABLE IF NOT EXISTS identity_registry (
                        peer_id BLOB PRIMARY KEY,
                        key_bundle BLOB NOT NULL
                    );
                    CREATE TABLE IF NOT EXISTS prekey_registry (
                        peer_id BLOB NOT NULL,
                        pre_key BLOB NOT NULL,
                        used INTEGER NOT NULL DEFAULT 0
                    );
                    ",
                )
                .map_err(de::Error::custom)?;
                Ok(RegistryState::new(Arc::new(Mutex::new(conn))))
            }
        }
        deserializer.deserialize_unit(RegistryStateVisitor)
    }
}

// ── Registry (trait carrier) ──────────────────────────────────────────────

/// Unit struct that carries the [`IdentityRegistry`] and [`PreKeyRegistry`]
/// trait implementations backed by [`RegistryState`].
#[derive(Debug, Serialize, Deserialize)]
pub struct Registry;

// ── IdentityRegistry ───────────────────────────────────────────────────────

impl IdentityRegistry<PeerId, RegistryState> for Registry {
    type Error = RegistryError;

    fn identity_key(y: &RegistryState, id: &PeerId) -> Result<Option<PublicKey>, Self::Error> {
        let peer_bytes = id.0.as_bytes().to_vec();
        let conn = y.conn.lock().unwrap();

        let bundle_bytes: Option<Vec<u8>> = conn
            .query_row(
                "SELECT key_bundle FROM identity_registry WHERE peer_id = ?1",
                params![peer_bytes],
                |row| row.get(0),
            )
            .optional()?;

        match bundle_bytes {
            Some(bytes) => {
                let bundle: OneTimeKeyBundle = postcard::from_bytes(&bytes)?;
                Ok(Some(*bundle.identity_key()))
            }
            None => Ok(None),
        }
    }
}

// ── PreKeyRegistry ─────────────────────────────────────────────────────────

impl PreKeyRegistry<PeerId, OneTimeKeyBundle> for Registry {
    type State = RegistryState;
    type Error = RegistryError;

    fn key_bundle(
        y: Self::State,
        id: &PeerId,
    ) -> Result<(Self::State, Option<OneTimeKeyBundle>), Self::Error> {
        let peer_bytes = id.0.as_bytes().to_vec();
        // Scope the mutex guard so it's dropped before we return `y`.
        let bundle = {
            let conn = y.conn.lock().unwrap();

            // Find the first unused pre-key bundle (lexicographically by rowid).
            let row: Option<(i64, Vec<u8>)> = conn
                .query_row(
                    "SELECT rowid, pre_key \
                     FROM prekey_registry \
                     WHERE peer_id = ?1 AND used = 0 \
                     ORDER BY rowid LIMIT 1",
                    params![peer_bytes],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;

            match row {
                Some((rowid, bundle_bytes)) => {
                    // Mark as used.
                    conn.execute(
                        "UPDATE prekey_registry SET used = 1 WHERE rowid = ?1",
                        params![rowid],
                    )?;
                    let bundle: OneTimeKeyBundle = postcard::from_bytes(&bundle_bytes)?;
                    Some(bundle)
                }
                None => None,
            }
        }; // MutexGuard dropped here

        Ok((y, bundle))
    }
}

// ── Error type ─────────────────────────────────────────────────────────────

/// Errors that can occur during registry operations.
#[derive(Debug)]
pub enum RegistryError {
    /// SQLite database error.
    Database(rusqlite::Error),
    /// Serialization or deserialization error (postcard).
    Serialization(postcard::Error),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::Database(e) => write!(f, "database error: {e}"),
            RegistryError::Serialization(e) => write!(f, "serialization error: {e}"),
        }
    }
}

impl std::error::Error for RegistryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RegistryError::Database(e) => Some(e),
            RegistryError::Serialization(e) => Some(e),
        }
    }
}

impl From<rusqlite::Error> for RegistryError {
    fn from(e: rusqlite::Error) -> Self {
        RegistryError::Database(e)
    }
}

impl From<postcard::Error> for RegistryError {
    fn from(e: postcard::Error) -> Self {
        RegistryError::Serialization(e)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use p2panda_encryption::crypto::x25519::SecretKey;
    use p2panda_encryption::crypto::xeddsa::xeddsa_sign;
    use p2panda_encryption::crypto::Rng;
    use p2panda_encryption::key_bundle::{Lifetime, OneTimePreKey, PreKey};
    use p2panda_encryption::traits::{IdentityRegistry, PreKeyRegistry};
    use rusqlite::Connection;

    use super::*;
    use crate::group_encryption::types::PeerId;

    /// Helper: create a fresh in-memory `RegistryState` with the registry
    /// tables already created.
    fn make_state() -> RegistryState {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS identity_registry (
                peer_id BLOB PRIMARY KEY,
                key_bundle BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prekey_registry (
                peer_id BLOB NOT NULL,
                pre_key BLOB NOT NULL,
                used INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .unwrap();
        RegistryState::new(Arc::new(Mutex::new(conn)))
    }

    /// Helper: generate a valid `OneTimeKeyBundle` for testing.
    fn make_bundle(rng: &Rng) -> OneTimeKeyBundle {
        let secret_key = SecretKey::from_rng(rng).unwrap();
        let identity_key = secret_key.verifying_key().unwrap();

        let signed_prekey_secret = SecretKey::from_rng(rng).unwrap();
        let signed_prekey = PreKey::new(
            signed_prekey_secret.verifying_key().unwrap(),
            Lifetime::default(),
        );
        let prekey_signature = xeddsa_sign(signed_prekey.as_bytes(), &secret_key, &rng).unwrap();

        let onetime_prekey_secret = SecretKey::from_rng(rng).unwrap();
        let onetime_prekey = OneTimePreKey::new(onetime_prekey_secret.verifying_key().unwrap(), 1);

        OneTimeKeyBundle::new(
            identity_key,
            signed_prekey,
            prekey_signature,
            Some(onetime_prekey),
        )
    }

    /// Helper: generate a PeerId for testing.
    fn make_peer(_seed: u64) -> PeerId {
        let sk = iroh::SecretKey::generate();
        PeerId::from(sk.public())
    }

    #[test]
    fn test_insert_identity_and_lookup() {
        let state = make_state();
        let rng = Rng::default();
        let peer = make_peer(1);
        let bundle = make_bundle(&rng);
        let expected_identity_key = *bundle.identity_key();

        // Insert the bundle.
        state.insert_identity(&peer, &bundle).unwrap();

        // Lookup via IdentityRegistry trait.
        let result = Registry::identity_key(&state, &peer).unwrap();
        assert_eq!(result, Some(expected_identity_key));
    }

    #[test]
    fn test_identity_lookup_missing() {
        let state = make_state();
        let peer = make_peer(99);

        let result = Registry::identity_key(&state, &peer).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_insert_pre_keys_and_consume() {
        let state = make_state();
        let rng = Rng::default();
        let peer = make_peer(2);

        // Generate three pre-key bundles.
        let bundle1 = make_bundle(&rng);
        let bundle2 = make_bundle(&rng);
        let bundle3 = make_bundle(&rng);

        // Insert them.
        state
            .insert_pre_keys(&peer, &[bundle1.clone(), bundle2.clone(), bundle3.clone()])
            .unwrap();

        // Consume one at a time via the PreKeyRegistry trait.
        let (state, first) = Registry::key_bundle(state, &peer).unwrap();
        assert!(first.is_some(), "first pre-key should be available");

        let (state, second) = Registry::key_bundle(state, &peer).unwrap();
        assert!(second.is_some(), "second pre-key should be available");

        let (state, third) = Registry::key_bundle(state, &peer).unwrap();
        assert!(third.is_some(), "third pre-key should be available");

        // All three consumed — next call should return None.
        let (_state, fourth) = Registry::key_bundle(state, &peer).unwrap();
        assert!(fourth.is_none(), "no pre-keys left");
    }

    #[test]
    fn test_insert_identity_replaces_existing() {
        let state = make_state();
        let rng = Rng::default();
        let peer = make_peer(3);
        let bundle = make_bundle(&rng);
        let expected_key = *bundle.identity_key();

        // Insert twice — second insert replaces the first.
        state.insert_identity(&peer, &make_bundle(&rng)).unwrap();
        state.insert_identity(&peer, &bundle).unwrap();

        let result = Registry::identity_key(&state, &peer).unwrap();
        assert_eq!(result, Some(expected_key));
    }

    #[test]
    fn test_pre_key_registry_unused_only() {
        let state = make_state();
        let rng = Rng::default();
        let peer = make_peer(4);

        // Insert one bundle and consume it.
        state.insert_pre_keys(&peer, &[make_bundle(&rng)]).unwrap();
        let (_, first) = Registry::key_bundle(state.clone(), &peer).unwrap();
        assert!(first.is_some(), "should consume the only pre-key");

        // No more pre-keys for this peer.
        let (_, second) = Registry::key_bundle(state, &peer).unwrap();
        assert!(second.is_none(), "no unused pre-keys remain");
    }
}
