//! Identity + PreKey manager: manages our own secret key material and
//! generates one-time pre-key bundles for X3DH handshakes.
//!
//! # Architecture
//!
//! [`KmgState`](crate::group_encryption::manager::KmgState) holds the local peer's x25519 identity key and all pre-key
//! state (long-term pre-keys + one-time secrets).  [`Manager`](crate::group_encryption::manager::Manager) is a unit
//! struct that carries the [`IdentityManager`] and [`PreKeyManager`] trait
//! implementations.
//!
//! The identity x25519 key is generated at startup (via
//! [`Manager::init_with_rng`](crate::group_encryption::manager::Manager::init_with_rng)) and should be persisted alongside the iroh
//! ed25519 secret key via serde.  New long-term pre-keys are generated with
//! [`PreKeyManager::rotate_prekey`](p2panda_encryption::traits::PreKeyManager::rotate_prekey); one-time bundles are created with
//! [`PreKeyManager::generate_onetime_bundle`](p2panda_encryption::traits::PreKeyManager::generate_onetime_bundle) and can be published via the
//! [`PreKeyRegistry`] for other peers to consume.
//!
//! [`IdentityManager`]: p2panda_encryption::traits::IdentityManager
//! [`PreKeyManager`]: p2panda_encryption::traits::PreKeyManager
//! [`PreKeyRegistry`]: p2panda_encryption::traits::PreKeyRegistry

use std::collections::HashMap;

use p2panda_encryption::crypto::x25519::{PublicKey, SecretKey, X25519Error};
use p2panda_encryption::crypto::xeddsa::{XEdDSAError, XSignature};
use p2panda_encryption::crypto::{Rng, RngError};
use p2panda_encryption::key_bundle::{
    Lifetime, LongTermKeyBundle, OneTimeKeyBundle, OneTimePreKey, OneTimePreKeyId, PreKey, PreKeyId,
};
use p2panda_encryption::traits::{IdentityManager, PreKeyManager};
use serde::{Deserialize, Serialize};

use super::types::PeerId;

// ── PreKeyBundleEntry ─────────────────────────────────────────────────────

/// Internal storage for a long-term pre-key and its signature.
///
/// Each entry holds the public [`PreKey`], the [`XSignature`] authenticating
/// it with the identity key, and the secret key material for DH operations.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PreKeyBundleEntry {
    prekey: PreKey,
    signature: XSignature,
    secret: SecretKey,
}

impl PreKeyBundleEntry {
    /// Generate a new signed pre-key bundle.
    fn new(
        identity_secret: &SecretKey,
        lifetime: Lifetime,
        rng: &Rng,
    ) -> Result<Self, ManagerError> {
        let secret = SecretKey::from_rng(rng)?;
        let prekey = PreKey::new(secret.verifying_key()?, lifetime);
        let signature = prekey.sign(identity_secret, rng)?;
        Ok(Self {
            prekey,
            signature,
            secret,
        })
    }

    fn id(&self) -> PreKeyId {
        *self.prekey.key()
    }
}

// ── KmgState ──────────────────────────────────────────────────────────────

/// Serializable state for the local peer's encryption key material.
///
/// # Fields
///
/// * `identity_secret` — x25519 secret key used as identity for X3DH/XEdDSA.
/// * `identity_key` — cached public key derived from `identity_secret`.
/// * `prekeys` — map of long-term pre-key id → bundle (public key + signature
///   + secret).
/// * `onetime_secrets` — consumed one-time pre-key secrets, linked to their
///   parent pre-key id.
/// * `onetime_next_id` — monotonically increasing counter for one-time pre-key
///   ids.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KmgState {
    identity_secret: SecretKey,
    identity_key: PublicKey,
    prekeys: HashMap<PreKeyId, PreKeyBundleEntry>,
    onetime_secrets: HashMap<OneTimePreKeyId, (PreKeyId, SecretKey)>,
    onetime_next_id: OneTimePreKeyId,
}

// ── Manager (trait carrier) ───────────────────────────────────────────────

/// Unit struct that carries the [`IdentityManager`] and [`PreKeyManager`]
/// trait implementations.
///
/// # Initialisation
///
/// ```ignore
/// use p2panda_encryption::crypto::Rng;
/// use p2panda_encryption::key_bundle::Lifetime;
///
/// let rng = Rng::default();
/// let state = Manager::init_with_rng(&rng)?;
/// let state = Manager::rotate_prekey(state, Lifetime::default(), &rng)?;
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct Manager;

// ── IdentityManager ───────────────────────────────────────────────────────

impl IdentityManager<KmgState> for Manager {
    fn identity_secret(y: &KmgState) -> &SecretKey {
        &y.identity_secret
    }
}

// ── PreKeyManager ─────────────────────────────────────────────────────────

impl PreKeyManager for Manager {
    type State = KmgState;
    type Error = ManagerError;

    fn prekey_secret<'a>(
        y: &'a Self::State,
        id: &'a PreKeyId,
    ) -> Result<&'a SecretKey, Self::Error> {
        match y.prekeys.get(id) {
            Some(entry) => Ok(&entry.secret),
            None => Err(ManagerError::UnknownPreKeySecret(*id)),
        }
    }

    fn rotate_prekey(
        mut y: Self::State,
        lifetime: Lifetime,
        rng: &Rng,
    ) -> Result<Self::State, Self::Error> {
        let entry = PreKeyBundleEntry::new(&y.identity_secret, lifetime, rng)?;
        y.prekeys.insert(entry.id(), entry);
        Ok(y)
    }

    fn prekey_bundle(y: &Self::State) -> Result<LongTermKeyBundle, Self::Error> {
        let latest = y
            .prekeys
            .values()
            .max_by_key(|e| e.prekey.lifetime())
            .ok_or(ManagerError::NoPreKeysAvailable)?;
        Ok(LongTermKeyBundle::new(
            y.identity_key,
            latest.prekey,
            latest.signature,
        ))
    }

    fn generate_onetime_bundle(
        mut y: Self::State,
        rng: &Rng,
    ) -> Result<(Self::State, OneTimeKeyBundle), Self::Error> {
        let latest = y
            .prekeys
            .values()
            .max_by_key(|e| e.prekey.lifetime())
            .ok_or(ManagerError::NoPreKeysAvailable)?;

        let onetime_secret = SecretKey::from_rng(rng)?;
        let onetime_key = OneTimePreKey::new(onetime_secret.verifying_key()?, y.onetime_next_id);

        // Sanity: should never overwrite an existing id.
        let existing = y
            .onetime_secrets
            .insert(onetime_key.id(), (latest.id(), onetime_secret));
        debug_assert!(existing.is_none(), "one-time pre-key id collision");

        let bundle = OneTimeKeyBundle::new(
            y.identity_key,
            latest.prekey,
            latest.signature,
            Some(onetime_key),
        );

        y.onetime_next_id += 1;
        Ok((y, bundle))
    }

    fn use_onetime_secret(
        mut y: Self::State,
        id: OneTimePreKeyId,
    ) -> Result<(Self::State, Option<SecretKey>), Self::Error> {
        match y.onetime_secrets.remove(&id) {
            Some((_, secret)) => Ok((y, Some(secret))),
            None => Err(ManagerError::UnknownOneTimeSecret(id)),
        }
    }
}

// ── Convenience helpers ───────────────────────────────────────────────────

impl Manager {
    /// Initialise [`KmgState`] with a freshly generated x25519 identity key.
    ///
    /// A first long-term pre-key is **not** generated here — call
    /// [`PreKeyManager::rotate_prekey`] after init to create one.
    pub fn init_with_rng(rng: &Rng) -> Result<KmgState, ManagerError> {
        let identity_secret = SecretKey::from_rng(rng)?;
        let identity_key = identity_secret.verifying_key()?;
        Ok(KmgState {
            identity_secret,
            identity_key,
            prekeys: HashMap::new(),
            onetime_secrets: HashMap::new(),
            onetime_next_id: 0,
        })
    }

    /// Return the local peer's [`PeerId`] (ed25519-based) from an iroh
    /// [`SecretKey`](iroh::SecretKey).
    ///
    /// This is **not** the x25519 identity key stored in [`KmgState`] —
    /// iroh uses Ed25519 for networking identity while p2panda-encryption
    /// uses X25519 for the encryption layer.
    pub fn peer_id(secret_key: &iroh::SecretKey) -> PeerId {
        PeerId::from(secret_key.public())
    }

    /// Remove all expired pre-key entries from `state`.
    ///
    /// Also removes one-time secrets whose parent pre-key no longer exists
    /// (e.g. because it expired and was already cleaned up).
    #[allow(clippy::manual_retain)]
    pub fn remove_expired(mut state: KmgState) -> KmgState {
        // Remove expired long-term pre-keys.
        state.prekeys = state
            .prekeys
            .into_iter()
            .filter(|(_, entry)| entry.prekey.verify_lifetime().is_ok())
            .collect();

        // Remove one-time secrets whose parent pre-key is gone.
        state.onetime_secrets = state
            .onetime_secrets
            .into_iter()
            .filter(|(_, (prekey_id, _))| state.prekeys.contains_key(prekey_id))
            .collect();

        state
    }
}

// ── Error type ────────────────────────────────────────────────────────────

/// Errors that can occur during key-manager operations.
#[derive(Debug)]
pub enum ManagerError {
    /// RNG failure.
    Rng(RngError),
    /// XEdDSA signing failure.
    XEdDSA(XEdDSAError),
    /// X25519 key operation failure.
    X25519(X25519Error),
    /// Could not find the requested one-time pre-key secret.
    UnknownOneTimeSecret(OneTimePreKeyId),
    /// Could not find the requested long-term pre-key secret.
    UnknownPreKeySecret(PreKeyId),
    /// No valid pre-keys available (all expired or none generated yet).
    NoPreKeysAvailable,
    /// Internal error (e.g. database error during setup).
    Internal(String),
}

impl std::fmt::Display for ManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManagerError::Rng(e) => write!(f, "RNG error: {e}"),
            ManagerError::XEdDSA(e) => write!(f, "XEdDSA error: {e}"),
            ManagerError::X25519(e) => write!(f, "X25519 error: {e}"),
            ManagerError::UnknownOneTimeSecret(id) => {
                write!(f, "could not find one-time pre-key secret with id {id}")
            }
            ManagerError::UnknownPreKeySecret(id) => {
                write!(f, "could not find pre-key secret with id {id}")
            }
            ManagerError::NoPreKeysAvailable => {
                write!(f, "no valid pre-keys available, rotate a new one")
            }
            ManagerError::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for ManagerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ManagerError::Rng(e) => Some(e),
            ManagerError::XEdDSA(e) => Some(e),
            ManagerError::X25519(e) => Some(e),
            ManagerError::UnknownOneTimeSecret(_)
            | ManagerError::UnknownPreKeySecret(_)
            | ManagerError::NoPreKeysAvailable
            | ManagerError::Internal(_) => None,
        }
    }
}

impl From<RngError> for ManagerError {
    fn from(e: RngError) -> Self {
        ManagerError::Rng(e)
    }
}

impl From<XEdDSAError> for ManagerError {
    fn from(e: XEdDSAError) -> Self {
        ManagerError::XEdDSA(e)
    }
}

impl From<X25519Error> for ManagerError {
    fn from(e: X25519Error) -> Self {
        ManagerError::X25519(e)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use p2panda_encryption::crypto::x25519::SecretKey;
    use p2panda_encryption::crypto::Rng;
    use p2panda_encryption::key_bundle::Lifetime;
    use p2panda_encryption::traits::{IdentityManager, KeyBundle, PreKeyManager};

    use super::*;

    /// Helper: initialise a fresh KmgState with an initial pre-key.
    fn make_state() -> KmgState {
        let rng = Rng::default();
        let mut state = Manager::init_with_rng(&rng).unwrap();
        state = Manager::rotate_prekey(state, Lifetime::default(), &rng).unwrap();
        state
    }

    #[test]
    fn test_identity_secret() {
        let rng = Rng::default();
        let state = Manager::init_with_rng(&rng).unwrap();

        let secret = Manager::identity_secret(&state);
        // Verify the secret produces the expected public key.
        let recovered_pk = secret.verifying_key().unwrap();
        assert_eq!(recovered_pk, state.identity_key);
    }

    #[test]
    fn test_pre_key_generation_and_bundle() {
        let rng = Rng::default();
        let mut state = Manager::init_with_rng(&rng).unwrap();

        // No pre-keys yet — bundle should fail.
        assert!(Manager::prekey_bundle(&state).is_err());

        // Rotate a new pre-key.
        state = Manager::rotate_prekey(state, Lifetime::default(), &rng).unwrap();

        // Now we should have a valid bundle.
        let bundle = Manager::prekey_bundle(&state).unwrap();
        assert_eq!(bundle.identity_key(), &state.identity_key);
        assert!(bundle.verify().is_ok());
    }

    #[test]
    fn test_one_time_bundle_generation() {
        let rng = Rng::default();
        let state = make_state();

        // Generate two one-time bundles.
        let (state, bundle1) = Manager::generate_onetime_bundle(state, &rng).unwrap();
        let (state, bundle2) = Manager::generate_onetime_bundle(state, &rng).unwrap();

        // Both bundles carry a one-time pre-key.
        assert!(bundle1.onetime_prekey().is_some());
        assert!(bundle2.onetime_prekey().is_some());

        // Both bundles are valid.
        assert!(bundle1.verify().is_ok());
        assert!(bundle2.verify().is_ok());

        // Identity key matches the state.
        assert_eq!(bundle1.identity_key(), &state.identity_key);
        assert_eq!(bundle2.identity_key(), &state.identity_key);

        // One-time pre-key ids are unique.
        assert_ne!(bundle1.onetime_prekey_id(), bundle2.onetime_prekey_id());

        // Consume both secrets.
        let (state, secret1) =
            Manager::use_onetime_secret(state, bundle1.onetime_prekey_id().unwrap()).unwrap();
        let (state, secret2) =
            Manager::use_onetime_secret(state, bundle2.onetime_prekey_id().unwrap()).unwrap();

        assert!(secret1.is_some());
        assert!(secret2.is_some());

        // Secrets match the public keys in the bundles.
        assert_eq!(
            bundle1.onetime_prekey().unwrap(),
            &secret1.unwrap().verifying_key().unwrap()
        );
        assert_eq!(
            bundle2.onetime_prekey().unwrap(),
            &secret2.unwrap().verifying_key().unwrap()
        );

        // Re-consuming the same id fails.
        assert!(
            Manager::use_onetime_secret(state.clone(), bundle1.onetime_prekey_id().unwrap())
                .is_err()
        );
        assert!(
            Manager::use_onetime_secret(state.clone(), bundle2.onetime_prekey_id().unwrap())
                .is_err()
        );

        // Unknown id fails.
        assert!(Manager::use_onetime_secret(state.clone(), 9999).is_err());
    }

    #[test]
    fn test_pre_key_secret_lookup() {
        let rng = Rng::default();
        let state = make_state();

        let bundle = Manager::prekey_bundle(&state).unwrap();
        let prekey_id = bundle.signed_prekey();

        // Look up the secret.
        let secret = Manager::prekey_secret(&state, prekey_id).unwrap();
        assert_eq!(
            &secret.verifying_key().unwrap(),
            prekey_id,
            "pre-key secret's public key should match"
        );

        // Unknown id fails.
        let unknown = SecretKey::from_rng(&rng).unwrap().verifying_key().unwrap();
        assert!(Manager::prekey_secret(&state, &unknown).is_err());
    }

    #[test]
    fn test_pre_key_rotation_and_expiry() {
        let rng = Rng::default();
        let mut state = make_state();

        // The initial bundle is valid.
        assert!(Manager::prekey_bundle(&state).is_ok());

        // Rotate a second pre-key with default lifetime (still valid).
        state = Manager::rotate_prekey(state, Lifetime::default(), &rng).unwrap();
        assert_eq!(
            state.prekeys.len(),
            2,
            "should have two pre-keys after rotation"
        );

        // Both should be valid.
        let bundle = Manager::prekey_bundle(&state).unwrap();
        assert!(bundle.verify().is_ok());

        // remove_expired should keep both since neither is expired.
        let state = Manager::remove_expired(state);
        assert_eq!(state.prekeys.len(), 2, "valid pre-keys survive cleanup");
    }

    #[test]
    fn test_serialize_roundtrip() {
        use postcard;
        let state = make_state();

        // Serialise to postcard.
        let bytes = postcard::to_allocvec(&state).unwrap();

        // Deserialise back.
        let deserialized: KmgState = postcard::from_bytes(&bytes).unwrap();

        // Identity keys should match.
        assert_eq!(state.identity_key, deserialized.identity_key);

        // Pre-keys and one-time state should survive roundtrip.
        let bundle = Manager::prekey_bundle(&deserialized).unwrap();
        assert!(bundle.verify().is_ok());
    }

    #[test]
    fn test_peer_id_helper() {
        use iroh::SecretKey as IrohSecretKey;

        let secret = IrohSecretKey::generate();
        let pk = secret.public();
        let peer_id = Manager::peer_id(&secret);

        // The PeerId should match the iroh PublicKey conversion.
        let expected: iroh::PublicKey = peer_id.into();
        assert_eq!(pk.as_bytes(), expected.as_bytes());
    }

    #[test]
    fn test_manager_sign_and_verify() {
        // Verify that KmgState can sign and verify (via pre-key bundles).
        let state = make_state();

        let bundle = Manager::prekey_bundle(&state).unwrap();

        // The bundle's identity key should match the state's.
        assert_eq!(bundle.identity_key(), &state.identity_key);

        // The bundle should verify (signature + lifetime check).
        assert!(bundle.verify().is_ok());

        // The identity_secret should match.
        let identity_secret = Manager::identity_secret(&state);
        assert_eq!(identity_secret.verifying_key().unwrap(), state.identity_key);
    }
}
