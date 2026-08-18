//! [`RemoteSharedFile`] and its `TryFrom<SharedFile>` mapping.
//!
//! The wire-friendly representation of a shared-file entry, plus the
//! conversion from the local [`SharedFile`](super::SharedFile) that strips
//! local-only fields (paths, row IDs, ticket data).

use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};

use super::{
    now_ms, timestamp_is_reasonable, valid_description_text, valid_identifier, valid_mime_type,
    SharedFile, MAX_COLLECTION_IDS, MAX_COLLECTION_ID_LENGTH, MAX_CONTENT_HASH_LENGTH,
    MAX_DESCRIPTION_LENGTH, MAX_DISPLAY_NAME_LENGTH, MAX_MIME_TYPE_LENGTH,
    MAX_SHARED_FILE_ID_LENGTH,
};

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
    /// Optional human-readable description. Tab, CR, and LF are allowed for
    /// ordinary formatting; all other control, line/paragraph-separator, and
    /// Unicode format characters are rejected by [`Self::validate`].
    #[serde(default)]
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
    /// Child file entries when this entry is a whole-directory (HashSeq
    /// collection) share.  A non-empty `children` vec means this entry
    /// represents a folder: `content_hash` is the collection root hash and
    /// `display_name` is the root folder name.  Absent/empty for ordinary
    /// single-file shares.  This is the catalogue-model representation of a
    /// received collection as ONE entry with children (SENDME-01).
    #[serde(default)]
    pub children: Vec<RemoteSharedFile>,
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
            children: vec![],
        }
    }

    /// True when this entry represents a whole-directory (HashSeq
    /// collection) share with child entries.
    pub fn is_folder(&self) -> bool {
        !self.children.is_empty()
    }

    /// Return the child entries of a folder share, or an empty slice for a
    /// single-file entry.
    pub fn folder_children(&self) -> &[RemoteSharedFile] {
        &self.children
    }

    /// Validate every field against length, format, and content constraints.
    ///
    /// Returns `Ok(())` when all constraints pass, or `Err` with a
    /// description of the first violation found.
    pub fn validate(&self) -> Result<()> {
        self.validate_fields()?;
        // A received collection is represented as ONE catalogue entry with
        // child entries.  Child entries are validated recursively with a
        // depth bound so a hostile folder tree cannot force unbounded
        // recursion during validation.
        self.validate_children(0)?;
        Ok(())
    }

    /// Validate this entry's own fields (shared_file_id, display_name,
    /// description, mime_type, size, hashes, timestamps, collection_ids).
    /// Does not descend into [`Self::children`] — see [`Self::validate`].
    fn validate_fields(&self) -> Result<()> {
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
        if !valid_identifier(&self.shared_file_id) {
            return Err(n0_error::anyerr!(
                "shared_file_id contains characters outside [A-Za-z0-9._-]"
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
        if self.display_name == "."
            || self.display_name == ".."
            || self.display_name.chars().any(|ch| ch.is_control())
            || self.display_name.contains('/')
            || self.display_name.contains('\\')
        {
            return Err(n0_error::anyerr!("display_name contains unsafe characters"));
        }

        // ── description (optional) ──────────────────────────────────────
        if let Some(ref desc) = self.description {
            if desc.len() > MAX_DESCRIPTION_LENGTH || !valid_description_text(desc) {
                return Err(n0_error::anyerr!(
                    "description is too long or contains disallowed control/format characters"
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
        if !valid_mime_type(&self.mime_type) {
            return Err(n0_error::anyerr!(
                "mime_type is not a valid lowercase MIME type"
            ));
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
        if !valid_identifier(&self.content_hash) {
            return Err(n0_error::anyerr!("content_hash contains unsafe characters"));
        }
        if self.size_bytes > crate::catalogue_limits::MAX_FILE_SIZE_BYTES {
            return Err(n0_error::anyerr!(
                "size_bytes exceeds the maximum allowed file size"
            ));
        }
        if !timestamp_is_reasonable(self.updated_at_ms) {
            return Err(n0_error::anyerr!("updated_at_ms is too far in the future"));
        }

        // ── version_number

        // ── collection_ids ──────────────────────────────────────────────
        if self.collection_ids.len() > MAX_COLLECTION_IDS {
            return Err(n0_error::anyerr!(
                "collection_ids count ({}) exceeds maximum of {}",
                self.collection_ids.len(),
                MAX_COLLECTION_IDS
            ));
        }
        for (i, id) in self.collection_ids.iter().enumerate() {
            if !valid_identifier(id) {
                return Err(n0_error::anyerr!(
                    "collection_ids[{}] contains unsafe characters",
                    i
                ));
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

    /// Recursively validate [`Self::children`], bounding nesting depth.
    ///
    /// A folder entry's children must individually pass the same validation
    /// rules as top-level entries.  Depth is bounded to prevent stack
    /// exhaustion from a maliciously deep folder tree; the total entry count
    /// is bounded by [`crate::catalogue_limits::MAX_ENTRIES_PER_COLLECTION`]
    /// across all levels.
    fn validate_children(&self, depth: usize) -> Result<()> {
        const MAX_COLLECTION_DEPTH: usize = 32;
        if self.children.is_empty() {
            return Ok(());
        }
        if depth >= MAX_COLLECTION_DEPTH {
            return Err(n0_error::anyerr!(
                "folder share exceeds maximum nesting depth of {MAX_COLLECTION_DEPTH}"
            ));
        }
        if self.children.len() > crate::catalogue_limits::MAX_ENTRIES_PER_COLLECTION {
            return Err(n0_error::anyerr!(
                "folder share has {} entries, exceeding maximum of {}",
                self.children.len(),
                crate::catalogue_limits::MAX_ENTRIES_PER_COLLECTION
            ));
        }
        for (i, child) in self.children.iter().enumerate() {
            child
                .validate_fields()
                .std_context(format!("invalid folder child [{i}]"))?;
            child
                .validate_children(depth + 1)
                .std_context(format!("invalid folder child [{i}]"))?;
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

        let content_hash = file.hash.map(hex::encode).unwrap_or_default();

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
            children: vec![],
        })
    }
}

impl TryFrom<SharedFile> for RemoteSharedFile {
    type Error = LocalPathError;

    fn try_from(file: SharedFile) -> std::result::Result<Self, Self::Error> {
        Self::try_from(&file)
    }
}
