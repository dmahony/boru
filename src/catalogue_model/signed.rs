//! [`SignedFileCatalogue`] — a signed collection of shared-file entries.
//!
//! Adds an owner signature over the serialised catalogue content so a peer
//! cannot tamper with any field (except the signature itself) without
//! invalidating the signature.

use iroh::{PublicKey, SecretKey};
use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};
use serde_byte_array::ByteArray;

use super::{timestamp_is_reasonable, FileCatalogueCollection, RemoteSharedFile, SIGNATURE_LEN};

// ── SignedFileCatalogue ──────────────────────────────────────────────────

/// A signed catalogue of remote shared files.
///
/// The catalogue content is serialised to a canonical byte representation
/// before signing, so tampering with any field (except the signature itself)
/// invalidates the signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedFileCatalogue {
    /// The peer who owns (and signed) this catalogue.
    pub owner_id: PublicKey,
    /// Monotonic revision counter — incremented whenever the set of files or
    /// their metadata changes.
    pub revision: u64,
    /// Timestamp of catalogue generation (ms since UNIX epoch).
    pub generated_at_ms: u64,
    /// Collections in this catalogue.
    pub collections: Vec<FileCatalogueCollection>,
    /// The shared files advertised in this catalogue.
    pub files: Vec<RemoteSharedFile>,
    /// Ed25519 signature over the serialised catalogue content.
    pub(crate) signature: ByteArray<SIGNATURE_LEN>,
}

impl SignedFileCatalogue {
    /// Create and sign a new catalogue on behalf of `secret_key`.
    pub fn sign(
        secret_key: &SecretKey,
        revision: u64,
        generated_at_ms: u64,
        collections: Vec<FileCatalogueCollection>,
        files: Vec<RemoteSharedFile>,
    ) -> Self {
        let owner_id = secret_key.public();
        let unsigned = Self {
            owner_id,
            revision,
            generated_at_ms,
            collections,
            files,
            signature: ByteArray::new([0u8; SIGNATURE_LEN]),
        };
        let payload = signing_payload(&unsigned);
        let signature = secret_key.sign(&payload);
        Self {
            signature: ByteArray::new(signature.to_bytes()),
            ..unsigned
        }
    }

    /// Verify that the signature is valid for the claimed `owner_id`.
    ///
    /// Returns `Ok(())` when the signature matches the serialised content,
    /// or an error describing the failure.  Pre-AUDIT-27 signatures over the
    /// bare tuple still verify during the migration window.
    pub fn verify(&self) -> Result<()> {
        let payload = signing_payload(self);
        let legacy = legacy_signing_payload(self);
        if !crate::protocol_signing::verify_canonical_or_legacy(
            &self.owner_id,
            self.signature.as_ref(),
            &payload,
            &legacy,
        ) {
            return Err(n0_error::anyerr!("catalogue signature verification failed"));
        }
        Ok(())
    }

    /// Validate all entries in the catalogue — files, collections, and signature.
    ///
    /// Calls [`RemoteSharedFile::validate`] on every file and
    /// [`FileCatalogueCollection::validate`] on every collection, then
    /// verifies the Ed25519 signature via [`verify`](Self::verify).
    pub fn validate(&self) -> Result<()> {
        if !timestamp_is_reasonable(self.generated_at_ms) {
            return Err(n0_error::anyerr!(
                "generated_at_ms is too far in the future"
            ));
        }
        // Validate each collection.
        for c in &self.collections {
            c.validate().std_context("collection validation failed")?;
        }
        // Validate each file.
        for f in &self.files {
            f.validate().std_context("file validation failed")?;
        }
        // Verify the signature.
        self.verify().std_context("signature validation failed")?;
        Ok(())
    }
}

/// Canonical protocol tag for signed file catalogues (BORU-AUDIT-27).
pub(crate) const CATALOGUE_PROTOCOL: &str = "boru/file-catalogue";
/// Version of the signed catalogue payload layout (BORU-AUDIT-27).
pub(crate) const CATALOGUE_VERSION: u16 = 1;

/// Produce the canonical payload that is signed / verified.
///
/// BORU-AUDIT-27: the canonical framing authenticates the owner identity, the
/// revision counter, the generation timestamp and the full catalogue content
/// — every security-relevant field.  The field order and content must remain
/// stable across versions.
pub(crate) fn signing_payload(catalogue: &SignedFileCatalogue) -> Vec<u8> {
    let digest = (
        &catalogue.owner_id,
        catalogue.revision,
        catalogue.generated_at_ms,
        &catalogue.collections,
        &catalogue.files,
    );
    crate::protocol_signing::canonical_signed_bytes(CATALOGUE_PROTOCOL, CATALOGUE_VERSION, &digest)
        .expect("postcard serialisation is infallible")
}

/// Legacy pre-AUDIT-27 signing bytes: bare postcard tuple without a domain
/// separator.
pub(crate) fn legacy_signing_payload(catalogue: &SignedFileCatalogue) -> Vec<u8> {
    let digest = (
        &catalogue.owner_id,
        catalogue.revision,
        catalogue.generated_at_ms,
        &catalogue.collections,
        &catalogue.files,
    );
    postcard::to_stdvec(&digest).expect("postcard serialisation is infallible")
}
