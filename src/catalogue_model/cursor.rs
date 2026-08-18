//! [`SignedCatalogueCursor`] — a signed pagination cursor.
//!
//! Encodes the requester, current revision, and next page index under an
//! owner signature so a peer cannot forge or tamper with pagination state.

use iroh::{PublicKey, SecretKey};
use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};
use serde_byte_array::ByteArray;

// ── SignedCatalogueCursor ────────────────────────────────────────────────

/// A signed pagination cursor that ties a specific page position to a
/// catalogue revision, owner, and requester.
///
/// The cursor is signed by the catalogue owner so clients cannot forge
/// cursor positions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedCatalogueCursor {
    /// Catalogue revision at cursor creation.
    pub revision: u64,
    /// Timestamp of the last file in the current page.
    pub last_updated_at_ms: u64,
    /// shared_file_id of the last file in the current page.
    pub last_file_id: String,
    /// The peer who signed this cursor (catalogue owner).
    pub owner_id: PublicKey,
    /// The peer this cursor was issued for.
    pub requester: PublicKey,
    /// Ed25519 signature over the cursor content.
    signature: ByteArray<{ iroh::Signature::LENGTH }>,
}

impl SignedCatalogueCursor {
    /// Create and sign a new cursor.
    pub fn sign(
        secret_key: &SecretKey,
        revision: u64,
        last_updated_at_ms: u64,
        last_file_id: &str,
        requester: PublicKey,
    ) -> Self {
        let owner_id = secret_key.public();
        let unsigned = Self {
            revision,
            last_updated_at_ms,
            last_file_id: last_file_id.to_string(),
            owner_id,
            requester,
            signature: ByteArray::new([0u8; iroh::Signature::LENGTH]),
        };
        let payload = cursor_signing_payload(&unsigned);
        let signature = secret_key.sign(&payload);
        Self {
            signature: ByteArray::new(signature.to_bytes()),
            ..unsigned
        }
    }

    /// Verify the cursor's signature against the claimed owner_id.
    pub fn verify(&self) -> Result<()> {
        let payload = cursor_signing_payload(self);
        let sig = iroh::Signature::from_bytes(&self.signature);
        self.owner_id
            .verify(&payload, &sig)
            .std_context("cursor signature verification failed")
    }

    /// Encode the cursor into an opaque string for wire transfer.
    pub fn encode(&self) -> String {
        let bytes = postcard::to_stdvec(self).expect("postcard serialisation is infallible");
        data_encoding::BASE64URL_NOPAD.encode(&bytes)
    }

    /// Decode a cursor from its wire string representation.
    pub fn decode(encoded: &str) -> Option<Self> {
        let bytes = data_encoding::BASE64URL_NOPAD
            .decode(encoded.as_bytes())
            .ok()?;
        postcard::from_bytes(&bytes).ok()
    }
}

/// Canonical signing payload for a [`SignedCatalogueCursor`].
fn cursor_signing_payload(cursor: &SignedCatalogueCursor) -> Vec<u8> {
    let digest = (
        cursor.revision,
        &cursor.last_file_id,
        cursor.last_updated_at_ms,
        cursor.requester,
    );
    postcard::to_stdvec(&digest).expect("postcard serialisation is infallible")
}
