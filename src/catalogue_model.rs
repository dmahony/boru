//! Remote-safe representation of shared file entries for wire transfer.
//!
//! [`RemoteSharedFile`] is the wire-friendly counterpart of
//! [`crate::user_profile::SharedFile`] — it strips local-only fields
//! (paths, database row IDs, blob tickets, permissions) so that file
//! metadata can be safely transmitted to remote peers.
//!
//! The module also provides [`SignedFileCatalogue`] (a signed collection
//! of [`RemoteSharedFile`] entries) and [`FileCatalogueCollection`]
//! (logical groupings of shared files).

use std::time::{SystemTime, UNIX_EPOCH};

use iroh::{PublicKey, SecretKey};
use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};
use serde_byte_array::ByteArray;

use crate::user_profile::SharedFile;

// ── Constants ────────────────────────────────────────────────────────────

/// Maximum length of a `shared_file_id`.
pub const MAX_SHARED_FILE_ID_LENGTH: usize = 256;

/// Maximum length of a `display_name`.
pub const MAX_DISPLAY_NAME_LENGTH: usize = 512;

/// Maximum length of a `description`.
pub const MAX_DESCRIPTION_LENGTH: usize = 1024;

/// Maximum length of a `mime_type` string.
pub const MAX_MIME_TYPE_LENGTH: usize = 128;

/// Maximum length of a `content_hash` string.
pub const MAX_CONTENT_HASH_LENGTH: usize = 128;

/// Maximum number of collection IDs per file.
pub const MAX_COLLECTION_IDS: usize = 256;

/// Maximum length of a single collection ID string.
pub const MAX_COLLECTION_ID_LENGTH: usize = 256;

const SIGNATURE_LEN: usize = iroh::Signature::LENGTH;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── RemoteSharedFile ─────────────────────────────────────────────────────

/// Remote-safe representation of a shared file for wire transfer.
///
/// Contains only metadata safe to share with remote peers — no local paths,
/// database row IDs, upload secrets, or blob tickets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteSharedFile {
    /// Stable identifier assigned by the publisher (e.g. a hash of local
    /// file metadata).  Distinct from `content_hash`, which is the
    /// blob-level content address.
    pub shared_file_id: String,
    /// Display name shown to peers (never a local path).
    pub display_name: String,
    /// Optional human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// MIME type of the file (e.g. `"application/pdf"`).
    pub mime_type: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Content hash used for blob identification / deduplication.
    pub content_hash: String,
    /// Monotonic version number incremented on each change to this entry.
    pub version_number: u32,
    /// Last-update timestamp (milliseconds since UNIX epoch).
    pub updated_at_ms: u64,
    /// Identifiers of the collections this file belongs to.
    #[serde(default)]
    pub collection_ids: Vec<String>,
}

impl RemoteSharedFile {
    /// Create a new [`RemoteSharedFile`] with default values for fields not
    /// explicitly passed.
    ///
    /// `shared_file_id` defaults to `content_hash` (callers should set it
    /// explicitly when the two differ via struct-literal syntax).
    /// `updated_at_ms` defaults to the current system time.
    /// `collection_ids` is populated from the optional `collection` parameter.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        content_hash: impl Into<String>,
        display_name: impl Into<String>,
        description: Option<String>,
        size_bytes: u64,
        mime_type: impl Into<String>,
        collection: Option<String>,
        version_number: u32,
    ) -> Self {
        let content_hash = content_hash.into();
        let display_name = display_name.into();
        let mime_type = mime_type.into();
        let collection_ids = match collection {
            Some(id) => vec![id],
            None => vec![],
        };

        Self {
            shared_file_id: content_hash.clone(),
            display_name,
            description,
            mime_type,
            size_bytes,
            content_hash,
            version_number,
            updated_at_ms: now_ms(),
            collection_ids,
        }
    }

    /// Validate every field against length, format, and content constraints.
    ///
    /// Returns `Ok(())` when all constraints pass, or `Err` with a
    /// description of the first violation found.
    pub fn validate(&self) -> Result<()> {
        // ── shared_file_id ──────────────────────────────────────────────
        if self.shared_file_id.is_empty() {
            return Err(n0_error::anyerr!("shared_file_id must not be empty"));
        }
        if self.shared_file_id.len() > MAX_SHARED_FILE_ID_LENGTH {
            return Err(n0_error::anyerr!(
                "shared_file_id exceeds maximum length of {} (got {})",
                MAX_SHARED_FILE_ID_LENGTH,
                self.shared_file_id.len()
            ));
        }
        if self.shared_file_id.contains('/') || self.shared_file_id.contains('\\') {
            return Err(n0_error::anyerr!(
                "shared_file_id must not contain path separators"
            ));
        }

        // ── display_name ────────────────────────────────────────────────
        if self.display_name.is_empty() {
            return Err(n0_error::anyerr!("display_name must not be empty"));
        }
        if self.display_name.len() > MAX_DISPLAY_NAME_LENGTH {
            return Err(n0_error::anyerr!(
                "display_name exceeds maximum length of {} (got {})",
                MAX_DISPLAY_NAME_LENGTH,
                self.display_name.len()
            ));
        }
        if self.display_name.contains('/') || self.display_name.contains('\\') {
            return Err(n0_error::anyerr!(
                "display_name must not contain path separators"
            ));
        }

        // ── description (optional) ──────────────────────────────────────
        if let Some(ref desc) = self.description {
            if desc.len() > MAX_DESCRIPTION_LENGTH {
                return Err(n0_error::anyerr!(
                    "description exceeds maximum length of {} (got {})",
                    MAX_DESCRIPTION_LENGTH,
                    desc.len()
                ));
            }
        }

        // ── mime_type ───────────────────────────────────────────────────
        if self.mime_type.is_empty() {
            return Err(n0_error::anyerr!("mime_type must not be empty"));
        }
        if self.mime_type.len() > MAX_MIME_TYPE_LENGTH {
            return Err(n0_error::anyerr!(
                "mime_type exceeds maximum length of {} (got {})",
                MAX_MIME_TYPE_LENGTH,
                self.mime_type.len()
            ));
        }
        if !self.mime_type.contains('/') {
            return Err(n0_error::anyerr!("mime_type must contain a '/' separator"));
        }

        // ── content_hash ────────────────────────────────────────────────
        if self.content_hash.is_empty() {
            return Err(n0_error::anyerr!("content_hash must not be empty"));
        }
        if self.content_hash.len() > MAX_CONTENT_HASH_LENGTH {
            return Err(n0_error::anyerr!(
                "content_hash exceeds maximum length of {} (got {})",
                MAX_CONTENT_HASH_LENGTH,
                self.content_hash.len()
            ));
        }

        // ── version_number ──────────────────────────────────────────────
        // Any u32 is valid; no constraint beyond the type.

        // ── collection_ids ──────────────────────────────────────────────
        if self.collection_ids.len() > MAX_COLLECTION_IDS {
            return Err(n0_error::anyerr!(
                "collection_ids count ({}) exceeds maximum of {}",
                self.collection_ids.len(),
                MAX_COLLECTION_IDS
            ));
        }
        for (i, id) in self.collection_ids.iter().enumerate() {
            if id.is_empty() {
                return Err(n0_error::anyerr!("collection_ids[{}] must not be empty", i));
            }
            if id.len() > MAX_COLLECTION_ID_LENGTH {
                return Err(n0_error::anyerr!(
                    "collection_ids[{}] exceeds maximum length of {} (got {})",
                    i,
                    MAX_COLLECTION_ID_LENGTH,
                    id.len()
                ));
            }
        }

        Ok(())
    }
}

// ── TryFrom<SharedFile> ──────────────────────────────────────────────────

/// Error returned when a local [`SharedFile`] cannot be safely converted to
/// a [`RemoteSharedFile`] because its path would leak local filesystem
/// information.
#[derive(Debug, Clone)]
pub struct LocalPathError {
    /// Human-readable explanation of what was rejected.
    pub reason: String,
}

impl std::fmt::Display for LocalPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "local path not allowed in remote-safe entry: {}",
            self.reason
        )
    }
}

impl std::error::Error for LocalPathError {}

/// Convert a local [`SharedFile`] into a remote-safe [`RemoteSharedFile`].
///
/// Returns [`LocalPathError`] when the file has an absolute path or a path
/// that escapes the shared folder via `..` components.
impl TryFrom<&SharedFile> for RemoteSharedFile {
    type Error = LocalPathError;

    fn try_from(file: &SharedFile) -> std::result::Result<Self, Self::Error> {
        // Reject absolute paths — they leak local filesystem structure.
        if file.path.is_absolute() {
            return Err(LocalPathError {
                reason: format!("absolute path is not remote-safe: {:?}", file.path),
            });
        }
        // Reject paths that escape the shared folder via parent-dir components.
        if file
            .path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(LocalPathError {
                reason: format!("path escapes shared folder: {:?}", file.path),
            });
        }

        let content_hash = file.hash.map(|h| hex::encode(h)).unwrap_or_default();

        Ok(Self {
            shared_file_id: file.id.clone(),
            display_name: file.filename.clone(),
            description: None,
            mime_type: file.mime_type.clone(),
            size_bytes: file.size,
            content_hash,
            version_number: 1,
            updated_at_ms: now_ms(),
            collection_ids: vec![],
        })
    }
}

impl TryFrom<SharedFile> for RemoteSharedFile {
    type Error = LocalPathError;

    fn try_from(file: SharedFile) -> std::result::Result<Self, Self::Error> {
        Self::try_from(&file)
    }
}

// ── FileCatalogueCollection ──────────────────────────────────────────────

/// A named group of shared files within a catalogue.
///
/// Used to organise files into logical collections (e.g. "photos",
/// "documents", "projects").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileCatalogueCollection {
    /// Unique identifier for this collection.
    pub collection_id: String,
    /// Human-readable display name.
    pub name: String,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ── SignedFileCatalogue ──────────────────────────────────────────────────

/// A signed catalogue of remote shared files.
///
/// The catalogue content is serialised to a canonical byte representation
/// before signing, so tampering with any field (except the signature itself)
/// invalidates the signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    signature: ByteArray<SIGNATURE_LEN>,
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
    /// or an error describing the failure.
    pub fn verify(&self) -> Result<()> {
        let payload = signing_payload(self);
        let sig = iroh::Signature::from_bytes(&self.signature);
        self.owner_id
            .verify(&payload, &sig)
            .std_context("catalogue signature verification failed")
    }
}

/// Produce the canonical payload that is signed / verified.
///
/// All fields except `signature` are serialised into a deterministic byte
/// sequence via postcard.  The order and content of the tuple must remain
/// stable across versions.
fn signing_payload(catalogue: &SignedFileCatalogue) -> Vec<u8> {
    let digest = (
        &catalogue.owner_id,
        catalogue.revision,
        catalogue.generated_at_ms,
        &catalogue.collections,
        &catalogue.files,
    );
    postcard::to_stdvec(&digest).expect("postcard serialisation is infallible")
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── RemoteSharedFile validation ─────────────────────────────────────

    #[test]
    fn valid_default_remote_shared_file() {
        let f = RemoteSharedFile::new("abc123", "photo.jpg", None, 42_000, "image/jpeg", None, 1);
        assert!(f.validate().is_ok());
    }

    #[test]
    fn empty_shared_file_id_rejected() {
        let f = RemoteSharedFile {
            shared_file_id: String::new(),
            ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
        };
        assert!(f.validate().is_err());
    }

    #[test]
    fn empty_display_name_rejected() {
        let f = RemoteSharedFile {
            display_name: String::new(),
            ..RemoteSharedFile::new("hash", "x", None, 100, "text/plain", None, 1)
        };
        assert!(f.validate().is_err());
    }

    #[test]
    fn path_separator_in_display_name_rejected() {
        let f = RemoteSharedFile {
            display_name: "../secret.txt".into(),
            ..RemoteSharedFile::new("hash", "x", None, 100, "text/plain", None, 1)
        };
        assert!(f.validate().is_err(), "path separator must be rejected");
    }

    #[test]
    fn path_separator_in_shared_file_id_rejected() {
        for sep in &["/", "\\"] {
            let f = RemoteSharedFile {
                shared_file_id: format!("sub{}dir/id", sep),
                ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
            };
            assert!(
                f.validate().is_err(),
                "shared_file_id containing '{}' must be rejected",
                sep
            );
        }
    }

    #[test]
    fn empty_mime_type_rejected() {
        let f = RemoteSharedFile {
            mime_type: String::new(),
            ..RemoteSharedFile::new("hash", "name", None, 100, "x", None, 1)
        };
        assert!(f.validate().is_err());
    }

    #[test]
    fn mime_type_without_slash_rejected() {
        let f = RemoteSharedFile {
            mime_type: "application".into(),
            ..RemoteSharedFile::new("hash", "name", None, 100, "x", None, 1)
        };
        assert!(f.validate().is_err());
    }

    #[test]
    fn empty_content_hash_rejected() {
        let f = RemoteSharedFile {
            content_hash: String::new(),
            ..RemoteSharedFile::new("x", "name", None, 100, "text/plain", None, 1)
        };
        assert!(f.validate().is_err());
    }

    #[test]
    fn field_length_limits_enforced() {
        // shared_file_id too long
        let long = "x".repeat(MAX_SHARED_FILE_ID_LENGTH + 1);
        let f = RemoteSharedFile {
            shared_file_id: long,
            ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
        };
        assert!(f.validate().is_err());

        // display_name too long
        let long = "x".repeat(MAX_DISPLAY_NAME_LENGTH + 1);
        let f = RemoteSharedFile {
            display_name: long,
            ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
        };
        assert!(f.validate().is_err());

        // mime_type too long
        let long = format!("{}/x", "a".repeat(MAX_MIME_TYPE_LENGTH));
        let f = RemoteSharedFile {
            mime_type: long,
            ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
        };
        assert!(f.validate().is_err());

        // content_hash too long
        let long = "x".repeat(MAX_CONTENT_HASH_LENGTH + 1);
        let f = RemoteSharedFile {
            content_hash: long,
            ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
        };
        assert!(f.validate().is_err());
    }

    #[test]
    fn description_length_limits_enforced() {
        let long = "x".repeat(MAX_DESCRIPTION_LENGTH + 1);
        let f = RemoteSharedFile {
            description: Some(long),
            ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
        };
        assert!(f.validate().is_err());
    }

    #[test]
    fn oversized_collection_list_rejected() {
        let ids: Vec<String> = (0..MAX_COLLECTION_IDS + 1)
            .map(|i| format!("col-{}", i))
            .collect();
        let f = RemoteSharedFile {
            collection_ids: ids,
            ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
        };
        assert!(
            f.validate().is_err(),
            "oversized collection list must be rejected"
        );
    }

    #[test]
    fn empty_collection_id_rejected() {
        let f = RemoteSharedFile {
            collection_ids: vec!["valid".into(), String::new(), "also-valid".into()],
            ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
        };
        assert!(f.validate().is_err());
    }

    #[test]
    fn valid_collection_ids_ok() {
        let f = RemoteSharedFile {
            collection_ids: vec!["photos".into(), "documents".into()],
            ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
        };
        assert!(f.validate().is_ok());
    }

    // ── TryFrom<SharedFile> ─────────────────────────────────────────────

    #[test]
    fn absolute_path_rejected() {
        let local = SharedFile {
            id: "file-1".into(),
            filename: "doc.pdf".into(),
            path: std::path::PathBuf::from("/etc/passwd"),
            size: 100,
            mime_type: "application/pdf".into(),
            modified_time: UNIX_EPOCH,
            hash: Some([1u8; 32]),
            blob_id: None,
            over_limit: false,
            extension_blocked: false,
        };
        let result = RemoteSharedFile::try_from(&local);
        assert!(
            result.is_err(),
            "absolute paths must not be convertible to remote-safe entries"
        );
        let err = result.unwrap_err();
        assert!(
            err.reason.contains("absolute"),
            "error should mention 'absolute': {}",
            err.reason
        );
    }

    #[test]
    fn parent_dir_path_rejected() {
        let local = SharedFile {
            id: "file-2".into(),
            filename: "leak.pdf".into(),
            path: std::path::PathBuf::from("../sensitive/data.pdf"),
            size: 200,
            mime_type: "application/pdf".into(),
            modified_time: UNIX_EPOCH,
            hash: Some([2u8; 32]),
            blob_id: None,
            over_limit: false,
            extension_blocked: false,
        };
        let result = RemoteSharedFile::try_from(&local);
        assert!(
            result.is_err(),
            "paths with parent-dir components must be rejected"
        );
    }

    #[test]
    fn relative_path_converts_ok() {
        let local = SharedFile {
            id: "file-3".into(),
            filename: "safe.pdf".into(),
            path: std::path::PathBuf::from("shared/safe.pdf"),
            size: 300,
            mime_type: "application/pdf".into(),
            modified_time: UNIX_EPOCH,
            hash: Some([3u8; 32]),
            blob_id: None,
            over_limit: false,
            extension_blocked: false,
        };
        let remote = RemoteSharedFile::try_from(&local).expect("relative path should convert");
        assert_eq!(remote.shared_file_id, "file-3");
        assert_eq!(remote.display_name, "safe.pdf");
        assert_eq!(remote.mime_type, "application/pdf");
        assert_eq!(remote.size_bytes, 300);
        assert_eq!(remote.content_hash, hex::encode([3u8; 32]));
        assert_eq!(remote.version_number, 1);
        assert!(remote.collection_ids.is_empty());
    }

    #[test]
    fn empty_path_converts_ok() {
        let local = SharedFile {
            id: "file-4".into(),
            filename: "empty_path.txt".into(),
            path: std::path::PathBuf::from(""),
            size: 50,
            mime_type: "text/plain".into(),
            modified_time: UNIX_EPOCH,
            hash: None,
            blob_id: None,
            over_limit: false,
            extension_blocked: false,
        };
        let remote = RemoteSharedFile::try_from(&local).expect("empty path should convert safely");
        assert!(remote.content_hash.is_empty(), "no hash -> empty string");
    }

    // ── SignedFileCatalogue ─────────────────────────────────────────────

    #[test]
    fn sign_and_verify_roundtrip() {
        let sk = SecretKey::generate();
        let files = vec![RemoteSharedFile::new(
            "hash1",
            "file1.txt",
            None,
            100,
            "text/plain",
            None,
            1,
        )];
        let catalogue = SignedFileCatalogue::sign(&sk, 1, 1000, vec![], files);
        assert!(
            catalogue.verify().is_ok(),
            "freshly-signed catalogue verifies"
        );
    }

    #[test]
    fn tampered_revision_fails_verification() {
        let sk = SecretKey::generate();
        let files = vec![RemoteSharedFile::new(
            "hash1",
            "file1.txt",
            None,
            100,
            "text/plain",
            None,
            1,
        )];
        let mut catalogue = SignedFileCatalogue::sign(&sk, 1, 1000, vec![], files);
        catalogue.revision = 9_999_999;
        assert!(
            catalogue.verify().is_err(),
            "tampered revision must fail verification"
        );
    }

    #[test]
    fn tampered_owner_id_fails_verification() {
        let sk = SecretKey::generate();
        let wrong_sk = SecretKey::generate();
        let files = vec![RemoteSharedFile::new(
            "hash1",
            "file1.txt",
            None,
            100,
            "text/plain",
            None,
            1,
        )];
        let mut catalogue = SignedFileCatalogue::sign(&sk, 1, 1000, vec![], files);
        catalogue.owner_id = wrong_sk.public();
        assert!(
            catalogue.verify().is_err(),
            "tampered owner_id must fail verification"
        );
    }

    #[test]
    fn tampered_files_fails_verification() {
        let sk = SecretKey::generate();
        let files = vec![RemoteSharedFile::new(
            "hash1",
            "file1.txt",
            None,
            100,
            "text/plain",
            None,
            1,
        )];
        let mut catalogue = SignedFileCatalogue::sign(&sk, 1, 1000, vec![], files);
        catalogue.files.push(RemoteSharedFile::new(
            "tampered",
            "injected.txt",
            None,
            999,
            "text/plain",
            None,
            2,
        ));
        assert!(
            catalogue.verify().is_err(),
            "tampered files list must fail verification"
        );
    }

    #[test]
    fn collections_roundtrip() {
        let sk = SecretKey::generate();
        let collections = vec![FileCatalogueCollection {
            collection_id: "col-1".into(),
            name: "Photos".into(),
            description: Some("My shared photos".into()),
        }];
        let files = vec![RemoteSharedFile::new(
            "hash1",
            "photo.jpg",
            None,
            5000,
            "image/jpeg",
            Some("col-1".into()),
            1,
        )];
        let catalogue = SignedFileCatalogue::sign(&sk, 2, 2000, collections, files);
        assert!(catalogue.verify().is_ok());
        assert_eq!(catalogue.revision, 2);
        assert_eq!(catalogue.collections.len(), 1);
        assert_eq!(catalogue.files.len(), 1);
        assert_eq!(catalogue.files[0].collection_ids, vec!["col-1".to_string()]);
    }
}
