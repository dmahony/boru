//! Type-conversion bridge between iroh and p2panda cryptographic types.
//!
//! Both iroh and p2panda use Ed25519 keys (32-byte public / 64-byte secret) and
//! BLAKE3 hashes (32 bytes), making byte-level conversion straightforward.
//!
//! # Newtype wrappers
//!
//! - [`PeerId`] — newtype around [`p2panda_core::VerifyingKey`] that implements
//!   [`IdentityHandle`] from p2panda-encryption and converts to/from
//!   [`iroh::PublicKey`].
//! - [`OpId`] — newtype around [`p2panda_core::Hash`] that implements
//!   [`OperationId`] from p2panda-encryption and converts to/from
//!   [`iroh_blobs::Hash`].

use std::fmt;

use iroh_blobs::Hash as IrohHash;
use p2panda_core::Hash as P2pandaHash;
use p2panda_core::VerifyingKey;
use p2panda_encryption::traits::{IdentityHandle, OperationId};
use serde::{Deserialize, Serialize};

// ── PublicKey ↔ VerifyingKey ────────────────────────────────────────────────

/// Convert an iroh [`PublicKey`](iroh::PublicKey) to a p2panda [`VerifyingKey`].
///
/// Both types store the compressed Ed25519 y-coordinate (32 bytes), so this is a
/// zero-copy byte copy.
pub fn public_key_to_verifying_key(pk: &iroh::PublicKey) -> VerifyingKey {
    VerifyingKey::from_bytes(pk.as_bytes()).expect("iroh PublicKey is always a valid ed25519 key")
}

/// Convert a p2panda [`VerifyingKey`] to an iroh [`PublicKey`](iroh::PublicKey).
pub fn verifying_key_to_iroh(vk: &VerifyingKey) -> iroh::PublicKey {
    let bytes = vk.as_bytes();
    iroh::PublicKey::from_bytes(bytes).expect("p2panda VerifyingKey is always a valid ed25519 key")
}

// ── SecretKey ↔ SigningKey ──────────────────────────────────────────────────

/// Convert an iroh [`SecretKey`](iroh_base::SecretKey) to a p2panda [`SigningKey`](p2panda_core::SigningKey).
pub fn secret_key_to_signing_key(sk: &iroh::SecretKey) -> p2panda_core::SigningKey {
    p2panda_core::SigningKey::from_bytes(&sk.to_bytes())
}

/// Convert a p2panda [`SigningKey`](p2panda_core::SigningKey) to an iroh [`SecretKey`](iroh_base::SecretKey).
pub fn signing_key_to_iroh(sk: &p2panda_core::SigningKey) -> iroh::SecretKey {
    iroh::SecretKey::from_bytes(sk.as_bytes())
}

// ── Hash ↔ Hash ─────────────────────────────────────────────────────────────

/// Convert an iroh [`Hash`](iroh_blobs::Hash) to a p2panda [`Hash`](p2panda_core::Hash).
///
/// Both are BLAKE3 hashes (32 bytes), so this is a zero-copy byte copy.
pub fn iroh_hash_to_p2panda(hash: &IrohHash) -> P2pandaHash {
    P2pandaHash::from_bytes(*hash.as_bytes())
}

/// Convert a p2panda [`Hash`](p2panda_core::Hash) to an iroh [`Hash`](iroh_blobs::Hash).
pub fn p2panda_hash_to_iroh(hash: &P2pandaHash) -> IrohHash {
    IrohHash::from_bytes(*hash.as_bytes())
}

// ── PeerId newtype ──────────────────────────────────────────────────────────

/// A peer identifier wrapping [`p2panda_core::VerifyingKey`].
///
/// Implements [`IdentityHandle`] (from p2panda-encryption) and converts to/from
/// [`iroh::PublicKey`] at zero cost.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct PeerId(pub VerifyingKey);

impl fmt::Debug for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PeerId")
            .field(&hex::encode(self.0.as_bytes()))
            .finish()
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PeerId({})", hex::encode(self.0.as_bytes()))
    }
}

// ── Serde impls (both types are 32-byte arrays) ─────────────────────────────

impl Serialize for PeerId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.0.as_bytes())
    }
}

impl<'de> Deserialize<'de> for PeerId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PeerIdVisitor;
        impl<'de> serde::de::Visitor<'de> for PeerIdVisitor {
            type Value = PeerId;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a 32-byte Ed25519 public key")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<PeerId, E> {
                let arr: [u8; 32] = v
                    .try_into()
                    .map_err(|_| serde::de::Error::invalid_length(v.len(), &"32 bytes"))?;
                VerifyingKey::from_bytes(&arr)
                    .map(PeerId)
                    .map_err(serde::de::Error::custom)
            }
        }
        deserializer.deserialize_bytes(PeerIdVisitor)
    }
}

impl IdentityHandle for PeerId {}

impl From<iroh::PublicKey> for PeerId {
    fn from(pk: iroh::PublicKey) -> Self {
        Self(public_key_to_verifying_key(&pk))
    }
}

impl From<PeerId> for iroh::PublicKey {
    fn from(pid: PeerId) -> Self {
        verifying_key_to_iroh(&pid.0)
    }
}

// ── OpId newtype ────────────────────────────────────────────────────────────

/// An operation identifier wrapping [`p2panda_core::Hash`].
///
/// Implements [`OperationId`] (from p2panda-encryption) and converts to/from
/// [`iroh_blobs::Hash`] at zero cost.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct OpId(pub P2pandaHash);

impl fmt::Debug for OpId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("OpId")
            .field(&hex::encode(self.0.as_bytes()))
            .finish()
    }
}

impl fmt::Display for OpId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OpId({})", hex::encode(self.0.as_bytes()))
    }
}

// ── Serde impls ──────────────────────────────────────────────────────────────

impl Serialize for OpId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.0.as_bytes())
    }
}

impl<'de> Deserialize<'de> for OpId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OpIdVisitor;
        impl<'de> serde::de::Visitor<'de> for OpIdVisitor {
            type Value = OpId;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a 32-byte BLAKE3 hash")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<OpId, E> {
                let arr: [u8; 32] = v
                    .try_into()
                    .map_err(|_| serde::de::Error::invalid_length(v.len(), &"32 bytes"))?;
                Ok(OpId(P2pandaHash::from_bytes(arr)))
            }
        }
        deserializer.deserialize_bytes(OpIdVisitor)
    }
}

impl OperationId for OpId {}

impl From<IrohHash> for OpId {
    fn from(hash: IrohHash) -> Self {
        Self(iroh_hash_to_p2panda(&hash))
    }
}

impl From<OpId> for IrohHash {
    fn from(oid: OpId) -> Self {
        p2panda_hash_to_iroh(&oid.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_public_key_roundtrip() {
        let sk = iroh::SecretKey::generate();
        let pk = sk.public();

        let vk = public_key_to_verifying_key(&pk);
        let pk_back = verifying_key_to_iroh(&vk);

        assert_eq!(pk.as_bytes(), pk_back.as_bytes(), "PublicKey roundtrip");
    }

    #[test]
    fn test_secret_key_roundtrip() {
        let sk = iroh::SecretKey::generate();

        let p2p_sk = secret_key_to_signing_key(&sk);
        let sk_back = signing_key_to_iroh(&p2p_sk);

        assert_eq!(sk.to_bytes(), sk_back.to_bytes(), "SecretKey roundtrip");
    }

    #[test]
    fn test_hash_roundtrip() {
        let data = b"hello world";
        let hash = blake3::hash(data);
        let iroh_hash = IrohHash::from_bytes(hash.into());

        let p2p_hash = iroh_hash_to_p2panda(&iroh_hash);
        let iroh_hash_back = p2panda_hash_to_iroh(&p2p_hash);

        assert_eq!(
            iroh_hash.as_bytes(),
            iroh_hash_back.as_bytes(),
            "Hash roundtrip"
        );
    }

    #[test]
    fn test_peer_id_from_iroh() {
        let sk = iroh::SecretKey::generate();
        let pk = sk.public();

        let peer_id = PeerId::from(pk);
        let pk_back: iroh::PublicKey = peer_id.into();

        assert_eq!(pk.as_bytes(), pk_back.as_bytes(), "PeerId roundtrip");
    }

    #[test]
    fn test_op_id_from_iroh() {
        let data = b"test operation";
        let hash = blake3::hash(data);
        let iroh_hash = IrohHash::from_bytes(hash.into());

        let op_id = OpId::from(iroh_hash);
        let hash_back: IrohHash = op_id.into();

        assert_eq!(iroh_hash.as_bytes(), hash_back.as_bytes(), "OpId roundtrip");
    }

    #[test]
    fn test_identity_handle_trait() {
        // Verify PeerId: IdentityHandle is implemented
        fn requires_identity_handle<T: IdentityHandle>() {}
        requires_identity_handle::<PeerId>();
    }

    #[test]
    fn test_operation_id_trait() {
        // Verify OpId: OperationId is implemented
        fn requires_operation_id<T: OperationId>() {}
        requires_operation_id::<OpId>();
    }
}
