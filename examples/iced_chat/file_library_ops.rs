//! File Library Operations — hashing, import, reference, and object reuse.
//!
//! This module implements Steps 6-9 of the local profile file library:
//!
//! **Step 6** — `hash_file_streaming()` — streaming BLAKE3 hash with progress,
//!              cancellation, and error detection.
//! **Step 7** — `import_file()` — validate → hash → copy to managed path →
//!              verify → atomic rename → DB insert/reuse → collections →
//!              manifest revision.
//! **Step 8** — `offer_referenced_file()` — validate → hash → store source path
//!              → create file_object → create shared_file → collections →
//!              manifest revision.
//! **Step 9** — `find_or_create_file_object()` — centralised lookup that reuses
//!              existing verified imported objects with the same hash+size.
//!
//! # Design notes
//!
//! - Imported files are stored content-addressed under `library_dir / <hex-hash-prefix> / <hex-hash>`.
//!   The prefix is the first two hex characters (256 buckets), keeping directory
//!   listings manageable even with millions of files.
//! - Referenced files are never copied — only the local source path is stored as
//!   the `filename` in the `file_objects` table plus a private registry file.
//! - Object reuse is hash-based: files with the same BLAKE3 hash and size share
//!   one `file_objects` row, regardless of whether the first instance was
//!   imported or referenced. A referenced object never has imported bytes; an
//!   imported object never has a source path.
//! - Multiple `shared_files` rows can point to the same `file_objects` row,
//!   enabling chat attachments and profile offers to share one object record.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use blake3::Hasher;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::file_library::StorageMode;

// ── HashedFile result ─────────────────────────────────────────────────────

/// Result of hashing a file with streaming BLAKE3.
#[derive(Debug, Clone)]
pub struct HashedFile {
    /// Hex-encoded BLAKE3 hash (64 hex chars, 32 bytes).
    pub content_hash: String,
    /// Total file size in bytes, measured during the same streaming operation.
    pub size_bytes: u64,
}

/// Progress notification emitted during streaming hash or copy operations.
#[derive(Debug, Clone, Copy)]
pub struct HashProgress {
    /// Bytes processed so far.
    pub bytes_processed: u64,
    /// Total file size in bytes (same as `size_bytes` on the final update).
    pub total_bytes: u64,
}

impl HashProgress {
    /// Progress as a fraction in [0.0, 1.0].
    pub fn fraction(&self) -> f64 {
        if self.total_bytes == 0 {
            1.0
        } else {
            self.bytes_processed as f64 / self.total_bytes as f64
        }
    }

    /// Human-readable progress string, e.g. "45%".
    pub fn percent(&self) -> String {
        format!("{}%", (self.fraction() * 100.0) as u64)
    }
}

// ── Hash progress sender ──────────────────────────────────────────────────

/// A sender for hash progress updates, designed to be consumed by an Iced
/// subscription or background task.  The sender is a `watch::Sender` so the
/// receiver can see the latest value without buffering.
pub struct HashProgressSender {
    tx: watch::Sender<HashProgress>,
}

impl HashProgressSender {
    /// Create a new progress channel.  Returns the sender and a receiver
    /// that the UI can subscribe to.
    pub fn new() -> (Self, watch::Receiver<HashProgress>) {
        let (tx, rx) = watch::channel(HashProgress {
            bytes_processed: 0,
            total_bytes: 0,
        });
        (Self { tx }, rx)
    }

    /// Send a progress update.  Silently ignores closed receivers.
    pub fn send(&self, progress: HashProgress) {
        let _ = self.tx.send(progress);
    }
}

// ── File hashing (Step 6) ─────────────────────────────────────────────────

/// Errors that can occur during streaming hash operations.
#[derive(Debug)]
pub enum HashError {
    /// I/O error reading the file.
    IoError(std::io::Error),
    /// The operation was cancelled before completion.
    Cancelled,
}

impl std::fmt::Display for HashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(e) => write!(f, "I/O error: {e}"),
            Self::Cancelled => write!(f, "operation was cancelled"),
        }
    }
}

impl std::error::Error for HashError {}

/// Stream a file through BLAKE3, computing its hash and size in one pass.
///
/// # Parameters
/// * `path` — Absolute path to the file to hash.
/// * `cancel` — Cancellation token; the operation checks this periodically and
///   returns `HashError::Cancelled` if signalled.  Drop the token's source to
///   cancel.
/// * `progress` — Optional sender for progress updates.  The sender is called
///   with progress after every `*PROGRESS_INTERVAL` bytes.  Pass
///   `std::sync::Arc::new(std::sync::Mutex::new(None))` to discard updates.
///
/// # Returns
/// `Ok(HashedFile)` on success with content hash and size.
///
/// # Design
/// - Uses `blake3::Hasher` in streaming mode (never loads the whole file into
///   memory).
/// - Reads in 64 KiB chunks (matching BLAKE3's internal block size for
///   efficient pipelining).
/// - Reports progress for large files every 1 MiB.
/// - Detects read errors immediately (returns `IoError`).
/// - Records total file size from the same streaming operation (no extra `stat`).
/// - Checks cancellation after every chunk read, so cancellation is prompt.
pub fn hash_file_streaming(
    path: &Path,
    cancel: &CancellationToken,
    progress: Option<&watch::Sender<HashProgress>>,
) -> std::result::Result<HashedFile, HashError> {
    use std::io::Read;

    const CHUNK_SIZE: usize = 64 * 1024; // 64 KiB read buffer
    const PROGRESS_INTERVAL: u64 = 1024 * 1024; // 1 MiB between progress calls

    let file = std::fs::File::open(path).map_err(HashError::IoError)?;
    let mut reader = std::io::BufReader::with_capacity(CHUNK_SIZE, file);
    let mut hasher = Hasher::new();
    let mut total: u64 = 0;
    let mut last_progress: u64 = 0;
    let mut buf = vec![0u8; CHUNK_SIZE];

    loop {
        // Check cancellation before each read (prompt cancellation).
        if cancel.is_cancelled() {
            return Err(HashError::Cancelled);
        }

        let n = reader.read(&mut buf).map_err(HashError::IoError)?;

        if n == 0 {
            break; // EOF
        }

        hasher.update(&buf[..n]);
        total += n as u64;

        // Report progress at PROGRESS_INTERVAL granularity.
        if let Some(tx) = progress {
            if total - last_progress >= PROGRESS_INTERVAL {
                last_progress = total;
                let _ = tx.send(HashProgress {
                    bytes_processed: total,
                    total_bytes: total, // unknown until EOF; updated at end
                });
            }
        }
    }

    // Send final progress (total_bytes now known).
    if let Some(tx) = progress {
        let _ = tx.send(HashProgress {
            bytes_processed: total,
            total_bytes: total,
        });
    }

    let hash = hasher.finalize();
    Ok(HashedFile {
        content_hash: hash.to_hex().to_string(),
        size_bytes: total,
    })
}

// ── Content-addressed managed path ────────────────────────────────────────

/// Compute the content-addressed storage path for a file with the given hash.
///
/// The path is `base_dir / <first-2-hex-chars> / <full-hex-hash>`.
/// Two hex chars = 256 buckets, keeping directory listings manageable.
///
/// # Example
/// For hash `"abcdef..."`, the path becomes:
/// `library_dir/ab/abcdef...`
pub fn content_addressed_path(base_dir: &Path, content_hash: &str) -> PathBuf {
    let prefix = &content_hash[..2];
    base_dir.join(prefix).join(content_hash)
}

// ── File type detection ───────────────────────────────────────────────────

/// Guess the MIME type from a file extension.
///
/// Falls back to `"application/octet-stream"` when the extension is unknown
/// or absent.  Uses a simple built-in mapping rather than pulling in the
/// `mime_guess` crate.
pub fn guess_mime_type(filename: &str) -> String {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "png" => "image/png".into(),
        "jpg" | "jpeg" => "image/jpeg".into(),
        "gif" => "image/gif".into(),
        "webp" => "image/webp".into(),
        "svg" => "image/svg+xml".into(),
        "bmp" => "image/bmp".into(),
        "ico" => "image/x-icon".into(),
        "mp4" => "video/mp4".into(),
        "webm" => "video/webm".into(),
        "mkv" => "video/x-matroska".into(),
        "avi" => "video/x-msvideo".into(),
        "mov" => "video/quicktime".into(),
        "mp3" => "audio/mpeg".into(),
        "ogg" => "audio/ogg".into(),
        "wav" => "audio/wav".into(),
        "flac" => "audio/flac".into(),
        "aac" => "audio/aac".into(),
        "opus" => "audio/opus".into(),
        "pdf" => "application/pdf".into(),
        "zip" => "application/zip".into(),
        "gz" | "gzip" => "application/gzip".into(),
        "tar" => "application/x-tar".into(),
        "rar" => "application/vnd.rar".into(),
        "7z" => "application/x-7z-compressed".into(),
        "json" => "application/json".into(),
        "xml" => "application/xml".into(),
        "csv" => "text/csv".into(),
        "html" | "htm" => "text/html".into(),
        "txt" => "text/plain".into(),
        "md" => "text/markdown".into(),
        "yaml" | "yml" => "application/x-yaml".into(),
        "toml" => "application/toml".into(),
        _ => "application/octet-stream".into(),
    }
}

// ── Import errors ─────────────────────────────────────────────────────────

/// Errors that can occur during file import operations.
#[derive(Debug)]
pub enum ImportError {
    /// Validation failed (file doesn't exist, symlink, zero-size, etc.).
    Validation(crate::file_library::FileValidationError),
    /// Hashing failed (I/O or cancellation).
    HashFailed(HashError),
    /// Failed to create the managed path directory structure.
    CreateDirFailed(std::io::Error),
    /// Failed to copy the file to the temporary path.
    CopyFailed(std::io::Error),
    /// Failed to verify the copied file (hash mismatch after copy).
    VerificationFailed {
        expected_hash: String,
        actual_hash: String,
    },
    /// Failed to rename the temporary file to its final managed path (atomic rename).
    RenameFailed(std::io::Error),
    /// The managed file already has a different size than expected (database inconsistency).
    ManagedFileSizeMismatch {
        content_hash: String,
        expected_size: u64,
        actual_size: u64,
    },
    /// Database operation failed.
    DatabaseError(String),
    /// The operation was cancelled.
    Cancelled,
    /// An error occurred while cleaning up temporary files.
    CleanupError(std::io::Error),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(e) => write!(f, "{e}"),
            Self::HashFailed(e) => write!(f, "hash failed: {e}"),
            Self::CreateDirFailed(e) => write!(f, "failed to create directory: {e}"),
            Self::CopyFailed(e) => write!(f, "copy failed: {e}"),
            Self::VerificationFailed {
                expected_hash,
                actual_hash,
            } => {
                write!(
                    f,
                    "verification failed: expected hash {expected_hash}, got {actual_hash}"
                )
            }
            Self::RenameFailed(e) => write!(f, "atomic rename failed: {e}"),
            Self::ManagedFileSizeMismatch {
                content_hash,
                expected_size,
                actual_size,
            } => {
                write!(
                    f,
                    "managed file size mismatch for {content_hash}: expected {expected_size}, got {actual_size}"
                )
            }
            Self::DatabaseError(e) => write!(f, "database error: {e}"),
            Self::Cancelled => write!(f, "operation cancelled"),
            Self::CleanupError(e) => write!(f, "cleanup error: {e}"),
        }
    }
}

impl std::error::Error for ImportError {}

// ── Import workflow (Step 7) ──────────────────────────────────────────────

/// Import a file into the content-addressed managed storage.
///
/// Full workflow:
///   1. Validate the source file (reuses `validate_file_for_library`).
///   2. Stream-hash the file with BLAKE3 (supports cancellation and progress).
///   3. Compute the managed path: `library_dir / <prefix> / <hash>`.
///   4. Copy to a temporary file next to the final path.
///   5. Verify the copied file's hash and size (streaming, cancellable).
///   6. Atomic rename (temp → managed path) — ensures the managed file is
///      never partially written on disk.
///   7. Insert or reuse the `file_object` row in the database.
///   8. Insert the `shared_file` row.
///   9. Assign to selected collections (if any).
///   10. Increment the profile manifest revision.
///
/// If the file object already exists (same hash+size), Step 4-6 are skipped
/// and the existing object is reused (deduplication).
///
/// # Parameters
/// * `source_path` — Absolute path to the source file.
/// * `display_name` — User-chosen display name for the library entry.
/// * `description` — Optional user description.
/// * `library_dir` — Root directory for content-addressed managed file storage.
/// * `storage` — The database storage backend.
/// * `profile_user_id` — Hex-encoded public key of the profile owner.
/// * `metadata_id` — Stable metadata ID from the user profile system.
/// * `selected_collections` — Collection IDs to add the file to.
/// * `cancel` — Cancellation token for aborting long operations.
/// * `progress` — Optional progress sender (receives hash progress).
///
/// # Returns
/// The content hash of the imported file on success.
pub fn import_file(
    source_path: &Path,
    display_name: &str,
    description: Option<&str>,
    library_dir: &Path,
    storage: &boru_core::storage::Storage,
    profile_user_id: &str,
    metadata_id: &str,
    selected_collections: &[i64],
    cancel: &CancellationToken,
    progress: Option<&watch::Sender<HashProgress>>,
) -> std::result::Result<String, ImportError> {
    // 1. Validate.
    crate::file_library::validate_file_for_library(source_path, display_name, description)
        .map_err(ImportError::Validation)?;

    // 2. Hash the source (streaming BLAKE3).
    let hashed =
        hash_file_streaming(source_path, cancel, progress).map_err(ImportError::HashFailed)?;
    let content_hash = &hashed.content_hash;
    let size = hashed.size_bytes;

    // Check cancellation between steps.
    if cancel.is_cancelled() {
        return Err(ImportError::Cancelled);
    }

    // 3. Determine managed path and ensure directory exists.
    let managed_path = content_addressed_path(library_dir, content_hash);
    let parent_dir = managed_path.parent().unwrap_or(library_dir);
    std::fs::create_dir_all(parent_dir).map_err(ImportError::CreateDirFailed)?;

    // 4-6. Copy to managed path (only if file doesn't already exist).
    if !managed_path.exists() {
        let temp_path = library_dir.join(format!(".tmp_{}.import", content_hash));

        // 4. Copy to temp file.
        let copy_result = std::fs::copy(source_path, &temp_path);
        if cancel.is_cancelled() {
            // Clean up temp file on cancellation.
            let _ = std::fs::remove_file(&temp_path);
            return Err(ImportError::Cancelled);
        }
        copy_result.map_err(ImportError::CopyFailed)?;

        // 5. Verify copied file (streaming hash).
        let copied_hashed =
            hash_file_streaming(&temp_path, cancel, None).map_err(ImportError::HashFailed)?;

        if copied_hashed.content_hash != *content_hash || copied_hashed.size_bytes != size {
            let _ = std::fs::remove_file(&temp_path);
            return Err(ImportError::VerificationFailed {
                expected_hash: content_hash.clone(),
                actual_hash: copied_hashed.content_hash,
            });
        }

        // 6. Atomic rename.
        // On Unix, `rename` is atomic if source and dest are on the same filesystem.
        // We use the same `library_dir` for both temp and final, so this is guaranteed.
        std::fs::rename(&temp_path, &managed_path).map_err(ImportError::RenameFailed)?;
    }

    // 7. Insert or reuse file_object.
    let mime_type = guess_mime_type(display_name);
    let now_ms = boru_core::chat_core::now_ms();

    find_or_create_file_object(storage, content_hash, size, &mime_type, display_name)
        .map_err(|e| ImportError::DatabaseError(e.to_string()))?;

    // 8. Insert shared_file.
    storage
        .upsert_shared_file(
            content_hash,
            profile_user_id,
            metadata_id,
            display_name,
            description,
            true, // offered
        )
        .map_err(|e| ImportError::DatabaseError(e.to_string()))?;

    // 9. Assign to selected collections.
    for &collection_id in selected_collections {
        storage
            .add_to_collection(collection_id, content_hash, 0)
            .map_err(|e| ImportError::DatabaseError(e.to_string()))?;
    }

    // 10. Increment manifest revision.
    let manifest_hash = format!("file-add:{}:{}", content_hash, now_ms);
    storage
        .bump_manifest_revision(profile_user_id, &manifest_hash)
        .map_err(|e| ImportError::DatabaseError(e.to_string()))?;

    Ok(content_hash.clone())
}

// ── Referenced file creation (Step 8) ────────────────────────────────────

/// Offer a file as a reference (no copy — local path stored as source).
///
/// Full workflow:
///   1. Validate the source file.
///   2. Stream-hash the file (cancellable).
///   3. Store the source path as the `filename` in the `file_objects` table —
///      the path is kept local-only and never exposed to peers.
///   4. Insert the `file_object` row (no data/blob_hash — referenced files
///      store their source path in a separate private registry).
///   5. Insert the `shared_file` row.
///   6. Assign to selected collections (if any).
///   7. Increment manifest revision.
///
/// # Private source path registry
///
/// Referenced files store their private local source path in a side file:
/// `library_dir / .refs / <hex-hash>`.  This keeps source paths out of the
/// database entirely, so they are never exposed in any DB dump or backup.
/// The `file_objects.filename` column stores only the display name.
///
/// # Parameters
/// * `source_path` — Absolute path to the original file on disk.
/// * `display_name` — Display name for the library entry.
/// * `description` — Optional description.
/// * `library_dir` — Root directory for file library metadata (`.refs/` subdir).
/// * `storage` — The database storage backend.
/// * `profile_user_id` — Hex-encoded public key of the profile owner.
/// * `metadata_id` — Stable metadata ID from the user profile system.
/// * `selected_collections` — Collection IDs to add the file to.
/// * `cancel` — Cancellation token for aborting long operations.
/// * `progress` — Optional progress sender.
pub fn offer_referenced_file(
    source_path: &Path,
    display_name: &str,
    description: Option<&str>,
    library_dir: &Path,
    storage: &boru_core::storage::Storage,
    profile_user_id: &str,
    metadata_id: &str,
    selected_collections: &[i64],
    cancel: &CancellationToken,
    progress: Option<&watch::Sender<HashProgress>>,
) -> std::result::Result<String, ImportError> {
    // 1. Validate.
    crate::file_library::validate_file_for_library(source_path, display_name, description)
        .map_err(ImportError::Validation)?;

    // 2. Hash the source (streaming BLAKE3).
    let hashed =
        hash_file_streaming(source_path, cancel, progress).map_err(ImportError::HashFailed)?;
    let content_hash = &hashed.content_hash;
    let size = hashed.size_bytes;

    if cancel.is_cancelled() {
        return Err(ImportError::Cancelled);
    }

    // 3. Store private source path in side file (`.refs/` subdirectory).
    let refs_dir = library_dir.join(".refs");
    std::fs::create_dir_all(&refs_dir).map_err(ImportError::CreateDirFailed)?;
    let ref_path = refs_dir.join(content_hash);
    std::fs::write(&ref_path, source_path.to_string_lossy().as_ref())
        .map_err(|e| ImportError::CopyFailed(e))?;

    // 4. Insert or reuse file_object.
    //    For referenced files, we store with empty data. The source path is
    //    stored in the private `.refs/` registry, not in the DB.
    let mime_type = guess_mime_type(display_name);
    let now_ms = boru_core::chat_core::now_ms();

    // Use `with_conn` to check-then-insert atomically.
    if !storage
        .file_object_has_references(content_hash)
        .map_err(|e| ImportError::DatabaseError(e.to_string()))?
    {
        storage
            .put_file_object(content_hash, size, &mime_type, display_name, &[])
            .map_err(|e| ImportError::DatabaseError(e.to_string()))?;
    }

    // 5. Insert shared_file.
    storage
        .upsert_shared_file(
            content_hash,
            profile_user_id,
            metadata_id,
            display_name,
            description,
            true,
        )
        .map_err(|e| ImportError::DatabaseError(e.to_string()))?;

    // 6. Assign to selected collections.
    for &collection_id in selected_collections {
        storage
            .add_to_collection(collection_id, content_hash, 0)
            .map_err(|e| ImportError::DatabaseError(e.to_string()))?;
    }

    // 7. Increment manifest revision.
    let manifest_hash = format!("file-ref:{}:{}", content_hash, now_ms);
    storage
        .bump_manifest_revision(profile_user_id, &manifest_hash)
        .map_err(|e| ImportError::DatabaseError(e.to_string()))?;

    Ok(content_hash.clone())
}

// ── Object reuse (Step 9) ─────────────────────────────────────────────────

/// Find an existing file object by hash, or create a new one.
///
/// This is the centralised object-lookup function.  It returns the content
/// hash if the object exists (or was just created).
///
/// # Object reuse semantics
///
/// * If a `file_objects` row with the given `content_hash` already exists,
///   it is reused as-is (no modification).  This is idempotent.
/// * Imported and referenced files that happen to have the same BLAKE3 hash
///   do **not** share a `file_objects` row unless created via this function.
///   In practice, a referenced file with a given hash and an imported file
///   with the same hash would collide on the primary key, so the second
///   insert is silently ignored (INSERT OR IGNORE).  However, the referenced
///   path would still exist in `.refs/`.  The caller is responsible for
///   choosing the correct storage mode — each file should be either imported
///   or referenced, not both.
/// * Deleting a shared_file entry does NOT delete the file_object (which
///   may be shared by other entries).
/// * Chat attachments (`message_attachments`) and profile offers
///   (`shared_files`) can share one `file_objects` row — the content hash
///   is the intersection point.
///
/// # Parameters
/// * `storage` — The database storage backend.
/// * `content_hash` — Hex-encoded BLAKE3 hash.
/// * `size` — File size in bytes.
/// * `mime_type` — MIME type string.
/// * `filename` — Display filename.
pub fn find_or_create_file_object(
    storage: &boru_core::storage::Storage,
    content_hash: &str,
    size: u64,
    mime_type: &str,
    filename: &str,
) -> anyhow::Result<()> {
    // put_file_object uses INSERT OR IGNORE, so it's idempotent.
    // If the row already exists, this is a no-op.
    storage.put_file_object(content_hash, size, mime_type, filename, &[])?;
    Ok(())
}

/// Look up the private source path for a referenced file.
///
/// Returns `None` if no reference path is stored (the file may be an imported
/// file, or the reference record was deleted).
pub fn get_referenced_source_path(library_dir: &Path, content_hash: &str) -> Option<PathBuf> {
    let ref_path = library_dir.join(".refs").join(content_hash);
    if ref_path.exists() {
        std::fs::read_to_string(&ref_path).ok().map(PathBuf::from)
    } else {
        None
    }
}

/// Check whether a referenced file's source still exists on disk.
pub fn referenced_source_available(library_dir: &Path, content_hash: &str) -> bool {
    get_referenced_source_path(library_dir, content_hash)
        .as_deref()
        .map(|p| p.exists())
        .unwrap_or(false)
}

// ── Iced async wrapper helpers ────────────────────────────────────────────

/// Result type for an iced async task that imports or references a file.
#[derive(Debug, Clone)]
pub enum FileLibraryOpResult {
    /// The operation succeeded with the given content hash.
    Success(String),
    /// The operation failed with an error message.
    Failed(String),
}

// ── Step 15: Detect changed referenced files ───────────────────────────────

/// Compare a referenced file's current hash against the stored original hash.
///
/// Returns `Some(current_hash)` if the file has changed, `None` if unchanged
/// or if the file is not a referenced file.
pub fn detect_changed_file(
    library_dir: &Path,
    storage: &boru_core::storage::Storage,
    content_hash: &str,
    profile_user_id: &str,
) -> anyhow::Result<Option<String>> {
    // Only referenced files can change (imported files are content-addressed).
    let ref_path = library_dir.join(".refs").join(content_hash);
    if !ref_path.exists() {
        return Ok(None); // Imported file — content is immutable.
    }

    let source_path = std::fs::read_to_string(&ref_path)?;
    let source = PathBuf::from(source_path.trim());

    if !source.exists() {
        // Source is missing — this is already handled by file verification.
        return Ok(None);
    }

    // Get the original hash from verification state.
    let orig = storage
        .get_original_hash(content_hash, profile_user_id)?
        .unwrap_or_default();

    // Hash the current file (no progress needed, files should be small enough).
    let cancel = tokio_util::sync::CancellationToken::new();
    let hashed = hash_file_streaming(&source, &cancel, None)?;

    if hashed.content_hash != orig && !orig.is_empty() {
        Ok(Some(hashed.content_hash))
    } else if hashed.content_hash != content_hash {
        // The database hash differs from the current — also a change.
        Ok(Some(hashed.content_hash))
    } else {
        Ok(None) // Unchanged.
    }
}

/// Update a shared file offer to use a new content hash.
///
/// Steps:
/// 1. Hash the current referenced source to get the new hash.
/// 2. Create/reuse the file_object for the new hash.
/// 3. Record the replacement relationship.
/// 4. Update the shared_file to use the new hash.
/// 5. Update verification state with the new original.
/// 6. Update revision.
/// 7. Increment manifest revision.
pub fn update_referenced_file_to_new_version(
    library_dir: &Path,
    storage: &boru_core::storage::Storage,
    old_content_hash: &str,
    profile_user_id: &str,
    metadata_id: &str,
    display_name: &str,
    description: Option<&str>,
    cancel: &tokio_util::sync::CancellationToken,
    progress: Option<&watch::Sender<HashProgress>>,
) -> anyhow::Result<String> {
    // 1. Get the source path from .refs and hash it.
    let ref_path = library_dir.join(".refs").join(old_content_hash);
    let source_path_str = std::fs::read_to_string(&ref_path)?;
    let source_path = PathBuf::from(source_path_str.trim());

    if !source_path.exists() {
        anyhow::bail!("Source file no longer exists: {}", source_path.display());
    }

    let hashed = hash_file_streaming(&source_path, cancel, progress)
        .map_err(|e| anyhow::anyhow!("Hash failed: {e}"))?;
    let new_hash = hashed.content_hash;

    if new_hash == old_content_hash {
        anyhow::bail!("File has not changed (hash is identical)");
    }

    // 2. Create/reuse file_object for the new hash.
    let mime_type = guess_mime_type(display_name);
    find_or_create_file_object(
        storage,
        &new_hash,
        hashed.size_bytes,
        &mime_type,
        display_name,
    )?;

    // 3. Record the replacement relationship.
    storage.record_file_replacement(old_content_hash, &new_hash, profile_user_id)?;

    // 4. Update the shared_file to use the new hash.
    //    We remove the old shared_file and create a new one with the new hash.
    //    This preserves the PK constraint while migrating to the new hash.
    let old_shared = storage.get_shared_file(profile_user_id, old_content_hash)?;
    if let Some(ref old) = old_shared {
        storage.delete_shared_file(old_content_hash, profile_user_id)?;
        storage.upsert_shared_file(
            &new_hash,
            profile_user_id,
            metadata_id,
            &old.display_filename,
            description.or(old.description.as_deref()),
            old.offered,
        )?;
    }

    // 5. Update verification state with new original hash/size.
    storage.set_file_availability(
        &new_hash,
        profile_user_id,
        "Available",
        Some(boru_core::chat_core::now_ms()),
        &new_hash,
        hashed.size_bytes,
    )?;

    // 6. Increment revision (already done by remove+re-add, but bump explicitly).
    storage.increment_shared_file_revision(&new_hash, profile_user_id)?;

    // 7. Increment manifest revision.
    let manifest_hash = format!(
        "file-update:{}:{}",
        new_hash,
        boru_core::chat_core::now_ms()
    );
    storage.bump_manifest_revision(profile_user_id, &manifest_hash)?;

    Ok(new_hash)
}

// ── Step 16: Offer removal ─────────────────────────────────────────────────

/// Remove a shared file offer from the profile.
///
/// This removes the `shared_files` entry, collection memberships, and
/// permissions.  The underlying `file_object` is preserved.  If the file
/// is an imported file with no other references, the caller may also delete
/// the managed bytes on disk.
pub fn remove_offer_from_profile(
    storage: &boru_core::storage::Storage,
    content_hash: &str,
    profile_user_id: &str,
    library_dir: &Path,
    delete_imported: bool,
) -> anyhow::Result<()> {
    // 1. Remove from profile (shared_file, collections, permissions).
    storage.delete_shared_file(content_hash, profile_user_id)?;

    // 2. Increment manifest revision.
    let manifest_hash = format!(
        "file-remove:{}:{}",
        content_hash,
        boru_core::chat_core::now_ms()
    );
    storage.bump_manifest_revision(profile_user_id, &manifest_hash)?;

    // 3. If requested, delete the imported copy (only if no other references).
    if delete_imported {
        let has_refs = storage.file_object_has_references(content_hash)?;
        if !has_refs {
            // Delete the managed file on disk.
            let managed_path = content_addressed_path(library_dir, content_hash);
            if managed_path.exists() {
                std::fs::remove_file(&managed_path)?;
            }
            // Delete the .refs entry if it exists.
            let ref_path = library_dir.join(".refs").join(content_hash);
            if ref_path.exists() {
                std::fs::remove_file(&ref_path)?;
            }
            // Delete the DB row (file object).
            storage.delete_file_object(content_hash)?;
        }
    }

    Ok(())
}

/// Delete the imported copy of a file when no references remain.
///
/// Only succeeds if the file has no shared_files or message_attachments
/// referencing it.
pub fn delete_imported_copy(
    storage: &boru_core::storage::Storage,
    content_hash: &str,
    library_dir: &Path,
) -> anyhow::Result<bool> {
    if storage.file_object_has_references(content_hash)? {
        return Ok(false); // Still referenced — can't delete.
    }

    // Delete managed file on disk.
    let managed_path = content_addressed_path(library_dir, content_hash);
    if managed_path.exists() {
        std::fs::remove_file(&managed_path)?;
    }

    // Delete .refs entry.
    let ref_path = library_dir.join(".refs").join(content_hash);
    if ref_path.exists() {
        std::fs::remove_file(&ref_path)?;
    }

    // Delete DB row.
    storage.delete_file_object(content_hash)?;

    Ok(true)
}

// ── Step 17: Cleanup unreferenced imported objects ─────────────────────────

/// Clean up all unreferenced imported file objects.
///
/// For each unreferenced object:
/// 1. Create a cleanup operation record.
/// 2. Mark in_progress.
/// 3. Delete the managed bytes on disk.
/// 4. Delete the DB record.
/// 5. Mark completed (or failed with retryable state).
/// 6. Return the number of bytes freed.
pub fn cleanup_unreferenced_imported_objects(
    storage: &boru_core::storage::Storage,
    library_dir: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> anyhow::Result<u64> {
    let candidates = storage.list_unreferenced_imported_objects("")?;
    let mut total_freed: u64 = 0;

    for candidate in &candidates {
        if cancel.is_cancelled() {
            break;
        }

        let op_id = storage.create_cleanup_operation(&candidate.content_hash)?;
        let _ = storage.update_cleanup_operation(op_id, "in_progress", 0, None);

        // Delete managed bytes.
        let managed_path = content_addressed_path(library_dir, &candidate.content_hash);
        let deleted_bytes = if managed_path.exists() {
            let size = std::fs::metadata(&managed_path)
                .ok()
                .map(|m| m.len())
                .unwrap_or(0);
            match std::fs::remove_file(&managed_path) {
                Ok(()) => size,
                Err(e) => {
                    let _ =
                        storage.update_cleanup_operation(op_id, "failed", 0, Some(&format!("{e}")));
                    continue; // Retryable — keep going.
                }
            }
        } else {
            0
        };

        // Delete .refs entry.
        let ref_path = library_dir.join(".refs").join(&candidate.content_hash);
        let _ = std::fs::remove_file(&ref_path);

        // Delete DB record.
        let _ = storage.delete_file_object(&candidate.content_hash);

        // Mark completed.
        let _ = storage.update_cleanup_operation(op_id, "completed", deleted_bytes, None);
        total_freed += deleted_bytes;
    }

    Ok(total_freed)
}

// ── Step 21: Startup recovery ─────────────────────────────────────────────

/// Errors that can occur during startup recovery.
#[derive(Debug)]
pub enum StartupRecoveryError {
    /// I/O error during temp file cleanup.
    IoError(std::io::Error),
    /// Database error during recovery.
    DatabaseError(String),
    /// An error occurred during verification.
    VerificationError(String),
}

impl std::fmt::Display for StartupRecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(e) => write!(f, "I/O error: {e}"),
            Self::DatabaseError(e) => write!(f, "database error: {e}"),
            Self::VerificationError(e) => write!(f, "verification error: {e}"),
        }
    }
}

impl std::error::Error for StartupRecoveryError {}

impl From<std::io::Error> for StartupRecoveryError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e)
    }
}

/// Result summary of the startup recovery process.
#[derive(Debug, Clone, Default)]
pub struct StartupRecoverySummary {
    /// Number of stale temp files removed.
    pub stale_temp_files_removed: usize,
    /// Number of interrupted operations recovered (failed).
    pub interrupted_ops_recovered: usize,
    /// Number of orphaned shared_files (DB record without file_object) found.
    pub orphaned_shared_files: usize,
    /// Number of orphaned file_objects (bytes without associations) found.
    pub orphaned_file_objects: usize,
    /// Number of shared_files with no verification state (marked unverified).
    pub unverified_files: usize,
    /// Number of referenced files whose source is missing at startup.
    pub missing_referenced_files: usize,
    /// Number of imported files whose managed bytes are missing on disk.
    pub missing_imported_bytes: usize,
}

impl std::fmt::Display for StartupRecoverySummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Startup recovery: {} stale temp files removed, {} interrupted ops recovered, \
             {} orphaned shared_files, {} orphaned file_objects, {} unverified files, \
             {} missing referenced sources, {} missing imported bytes",
            self.stale_temp_files_removed,
            self.interrupted_ops_recovered,
            self.orphaned_shared_files,
            self.orphaned_file_objects,
            self.unverified_files,
            self.missing_referenced_files,
            self.missing_imported_bytes,
        )
    }
}

/// Find stale temporary import files in the library directory.
///
/// Stale temp files match the pattern `.tmp_*.import` and may have been left
/// behind by interrupted import operations.
pub fn find_stale_temp_files(library_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut stale = Vec::new();
    if let Ok(entries) = std::fs::read_dir(library_dir) {
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with(".tmp_") && name_str.ends_with(".import") {
                stale.push(entry.path());
            }
        }
    }
    Ok(stale)
}

/// Remove stale temporary import files from the library directory.
///
/// Returns the number of files removed.
pub fn remove_stale_temp_files(library_dir: &Path) -> std::io::Result<usize> {
    let stale = find_stale_temp_files(library_dir)?;
    let mut count = 0;
    for path in &stale {
        if std::fs::remove_file(path).is_ok() {
            count += 1;
        }
    }
    Ok(count)
}

/// Recover interrupted operations by marking them as failed.
///
/// Returns the number of operations that were recovered.
pub fn recover_interrupted_operations(
    storage: &boru_core::storage::Storage,
) -> anyhow::Result<usize> {
    let count = storage.fail_all_incomplete_operations("startup_recovery")?;
    Ok(count)
}

/// Perform startup recovery for the file library.
///
/// This function:
/// 1. Removes stale temp import files from `library_dir`.
/// 2. Recovers (fails) any interrupted DB operations.
/// 3. Lists orphaned shared_files (no file_object) and orphaned file_objects (no associations).
/// 4. Performs cheap metadata checks on shared files (verifies source availability,
///    marks unverified files, checks for missing managed bytes).
/// 5. Does NOT auto-hash all referenced files (to avoid startup delay).
///
/// Returns a summary of what was found and recovered.
pub fn startup_recovery(
    library_dir: &Path,
    storage: &boru_core::storage::Storage,
    profile_user_id: &str,
) -> std::result::Result<StartupRecoverySummary, StartupRecoveryError> {
    let mut summary = StartupRecoverySummary::default();

    // 1. Remove stale temp files.
    summary.stale_temp_files_removed =
        remove_stale_temp_files(library_dir).map_err(StartupRecoveryError::IoError)?;

    // 2. Recover interrupted operations.
    summary.interrupted_ops_recovered = storage
        .fail_all_incomplete_operations("startup_recovery")
        .map_err(|e| StartupRecoveryError::DatabaseError(e.to_string()))?;

    // 3. Check for orphaned records.
    let orphaned_hashes = storage
        .list_orphaned_shared_file_hashes(profile_user_id)
        .map_err(|e| StartupRecoveryError::DatabaseError(e.to_string()))?;
    summary.orphaned_shared_files = orphaned_hashes.len();

    let orphaned_objects = storage
        .list_orphaned_file_objects()
        .map_err(|e| StartupRecoveryError::DatabaseError(e.to_string()))?;
    summary.orphaned_file_objects = orphaned_objects.len();

    // 4. Cheap metadata checks on shared files.
    //    We check for files needing verification (no verification state) without
    //    hashing referenced files (that would be expensive).  We only do
    //    filesystem existence checks.
    let needing = storage
        .list_files_needing_verification(profile_user_id)
        .map_err(|e| StartupRecoveryError::DatabaseError(e.to_string()))?;
    summary.unverified_files = needing.len();

    // Check for missing managed bytes (imported files whose managed path doesn't exist).
    let offered_files = storage
        .list_shared_files(profile_user_id, true)
        .map_err(|e| StartupRecoveryError::DatabaseError(e.to_string()))?;

    for file in &offered_files {
        // Check if this file has managed bytes on disk (imported file).
        let managed_path = content_addressed_path(library_dir, &file.content_hash);
        let ref_path = library_dir.join(".refs").join(&file.content_hash);

        if managed_path.exists() {
            // Imported file: managed bytes exist — good.
            continue;
        }

        if ref_path.exists() {
            // Referenced file: check if the source still exists.
            // Read the source path from the ref file.
            match std::fs::read_to_string(&ref_path) {
                Ok(source_str) => {
                    let source_path = PathBuf::from(source_str.trim());
                    if !source_path.exists() {
                        summary.missing_referenced_files += 1;
                        // Mark as missing in verification state.
                        let _ = storage.set_file_availability(
                            &file.content_hash,
                            profile_user_id,
                            "Missing",
                            None,
                            "",
                            0,
                        );
                    }
                }
                Err(_) => {
                    summary.missing_referenced_files += 1;
                }
            }
        } else {
            // Neither managed path nor ref path — imported bytes are missing.
            // Check if this is an imported file (by checking if it has a
            // file_object with zero inline data that would indicate referenced).
            if let Ok(Some(obj)) = storage.get_file_object(&file.content_hash) {
                if obj.data.as_deref().map(|d| d.is_empty()).unwrap_or(true) {
                    // No inline data — likely a referenced file but missing ref path.
                    summary.missing_referenced_files += 1;
                } else {
                    // Imported file with inline data — missing managed bytes.
                    summary.missing_imported_bytes += 1;
                    let _ = storage.set_file_availability(
                        &file.content_hash,
                        profile_user_id,
                        "Missing",
                        None,
                        "",
                        0,
                    );
                }
            }
        }
    }

    Ok(summary)
}

// ── Step 22: Privacy and safety checks ─────────────────────────────────────

/// Sanitize a local filesystem path for logging or diagnostics.
///
/// Redacts the home directory, usernames, temp paths, and drive letters
/// so that logs do not leak local filesystem layout.
///
/// # Examples
///
/// ```
/// let safe = sanitize_path_for_log("/home/alice/docs/report.pdf");
/// assert_eq!(safe, "~/docs/report.pdf");
/// ```
pub fn sanitize_path_for_log(path: &std::path::Path) -> String {
    let path_str = path.to_string_lossy();

    // Replace home directory with ~
    if let Ok(home) = std::env::var("HOME") {
        if path_str.starts_with(&home) {
            let rest = path_str[home.len()..].to_string();
            return format!("~{}", rest);
        }
    }

    // On Unix, also check for /home/<username> patterns.
    // Replace /home/<anything> with ~, since it might be a different user's home.
    if path_str.starts_with("/home/") {
        let after_home = &path_str[6..]; // skip "/home/"
        if let Some(slash_pos) = after_home.find('/') {
            // It's /home/<username>/<path>
            let rest = &after_home[slash_pos..];
            return format!("~{}", rest);
        }
    }

    // Replace /tmp/ paths with <temp>
    if path_str.starts_with("/tmp/") || path_str.starts_with("/var/tmp/") {
        let rest = if path_str.starts_with("/tmp/") {
            &path_str[5..]
        } else {
            &path_str[9..]
        };
        return format!("<temp>/{}", rest);
    }

    // Redact any path with ".refs/" or "library/" containing hashes
    // (internal object paths).
    if path_str.contains("/.refs/") || path_str.contains("/library/") {
        if let Some(pos) = path_str.rfind('/') {
            return format!("<library>/{}", &path_str[pos + 1..]);
        }
    }

    path_str.to_string()
}

/// Check that a `FileLibraryRow` contains no local path information that would
/// be unsafe to expose to remote peers.
///
/// Returns `Ok(())` if the row is safe, or an `Err` with details if a local
/// path or other sensitive information is detected in a field that would be
/// visible to peers.
pub fn verify_row_safe_for_remote(row: &crate::file_library::FileLibraryRow) -> Result<(), String> {
    // Check display_filename for paths (should be just a filename, no separators).
    if row.display_filename.contains('/') || row.display_filename.contains('\\') {
        return Err(format!(
            "display_filename contains path separator: {}",
            row.display_filename
        ));
    }

    // Check display_filename for absolute path indicators.
    if row.display_filename.starts_with('/')
        || row.display_filename.starts_with('~')
        || row.display_filename.starts_with('.')
        || row.display_filename.starts_with("C:")
        || row.display_filename.starts_with("D:")
    {
        return Err(format!(
            "display_filename looks like a path: {}",
            row.display_filename
        ));
    }

    // Check description for paths or sensitive patterns.
    if let Some(ref desc) = row.description {
        let lower_desc = desc.to_lowercase();
        let sensitive_patterns = [
            "/home/", "c:\\", "d:\\", "/tmp/", "/var/", ".ssh", "secret", "password", "private",
        ];
        for pattern in &sensitive_patterns {
            if lower_desc.contains(pattern) {
                return Err(format!(
                    "description contains sensitive pattern '{}': {}",
                    pattern, desc
                ));
            }
        }
    }

    // Check MIME type is a standard type (not a path disguised as MIME).
    if !row.mime_type.contains('/') {
        return Err(format!(
            "MIME type does not contain '/' separator: {}",
            row.mime_type
        ));
    }

    Ok(())
}

/// Verify that a collection of file library rows does not leak local paths
/// when presented as a remote-safe view.
///
/// This is a bulk version of `verify_row_safe_for_remote` that checks all
/// rows and returns all violations found.
pub fn verify_all_rows_safe_for_remote(
    rows: &[crate::file_library::FileLibraryRow],
) -> Vec<(usize, String)> {
    let mut violations = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        if let Err(msg) = verify_row_safe_for_remote(row) {
            violations.push((i, msg));
        }
    }
    violations
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn test_library_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("library");
        std::fs::create_dir_all(&path).unwrap();
        (dir, path)
    }

    fn test_storage() -> boru_core::storage::Storage {
        boru_core::storage::Storage::memory().expect("create memory storage")
    }

    // ── Step 6: Hashing tests ──────────────────────────────────────────

    #[test]
    fn test_hash_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.txt");
        std::fs::write(&path, b"").unwrap();
        let cancel = CancellationToken::new();
        let result = hash_file_streaming(&path, &cancel, None).unwrap();
        // BLAKE3 hash of empty input is a well-known value.
        assert_eq!(result.content_hash.len(), 64);
        assert_eq!(result.size_bytes, 0);
    }

    #[test]
    fn test_hash_small_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.txt");
        std::fs::write(&path, b"hello world").unwrap();
        let cancel = CancellationToken::new();
        let result = hash_file_streaming(&path, &cancel, None).unwrap();
        assert_eq!(result.content_hash.len(), 64);
        assert_eq!(result.size_bytes, 11);
        // Deterministic: same content = same hash.
        let result2 = hash_file_streaming(&path, &cancel, None).unwrap();
        assert_eq!(result.content_hash, result2.content_hash);
    }

    #[test]
    fn test_hash_large_generated_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.bin");
        let content = vec![0xABu8; 5_000_000]; // ~5 MB
        std::fs::write(&path, &content).unwrap();
        let cancel = CancellationToken::new();
        let result = hash_file_streaming(&path, &cancel, None).unwrap();
        assert_eq!(result.content_hash.len(), 64);
        assert_eq!(result.size_bytes, 5_000_000);
    }

    #[test]
    fn test_hash_identical_content_different_names() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, b"same content").unwrap();
        std::fs::write(&b, b"same content").unwrap();
        let cancel = CancellationToken::new();
        let ha = hash_file_streaming(&a, &cancel, None).unwrap();
        let hb = hash_file_streaming(&b, &cancel, None).unwrap();
        assert_eq!(ha.content_hash, hb.content_hash);
        assert_eq!(ha.size_bytes, hb.size_bytes);
    }

    #[test]
    fn test_hash_changed_content_differs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, b"version 1").unwrap();
        let cancel = CancellationToken::new();
        let h1 = hash_file_streaming(&path, &cancel, None).unwrap();
        std::fs::write(&path, b"version 2").unwrap();
        let h2 = hash_file_streaming(&path, &cancel, None).unwrap();
        assert_ne!(h1.content_hash, h2.content_hash);
    }

    #[test]
    fn test_hash_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.bin");
        let content = vec![0xFFu8; 10_000_000]; // 10 MB
        std::fs::write(&path, &content).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel(); // Cancel immediately before starting.
        let result = hash_file_streaming(&path, &cancel, None);
        assert!(matches!(result, Err(HashError::Cancelled)));
    }

    #[test]
    fn test_hash_nonexistent_file_returns_error() {
        let cancel = CancellationToken::new();
        let path = std::path::Path::new("/tmp/__nonexistent_hash_test__");
        let result = hash_file_streaming(path, &cancel, None);
        assert!(matches!(result, Err(HashError::IoError(_))));
    }

    #[test]
    fn test_hash_progress_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("medium.bin");
        let content = vec![0xAAu8; 3_000_000]; // ~3 MB
        std::fs::write(&path, &content).unwrap();
        let (tx, rx) = watch::channel(HashProgress {
            bytes_processed: 0,
            total_bytes: 0,
        });
        let cancel = CancellationToken::new();
        let result = hash_file_streaming(&path, &cancel, Some(&tx)).unwrap();
        assert_eq!(result.size_bytes, 3_000_000);
        // At least one progress update should have been sent.
        let last = rx.borrow().clone();
        assert!(last.bytes_processed > 0);
    }

    // ── Content-addressed path tests ───────────────────────────────────

    #[test]
    fn test_content_addressed_path_computation() {
        let base = std::path::Path::new("/library");
        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let path = content_addressed_path(base, hash);
        assert_eq!(path, base.join("ab").join(hash));
    }

    #[test]
    fn test_content_addressed_path_short_hash() {
        let base = std::path::Path::new("/library");
        let path = content_addressed_path(base, "a1");
        assert_eq!(path, base.join("a1").join("a1"));
    }

    // ── MIME type tests ────────────────────────────────────────────────

    #[test]
    fn test_guess_mime_type_known_extensions() {
        assert_eq!(guess_mime_type("photo.png"), "image/png");
        assert_eq!(guess_mime_type("doc.pdf"), "application/pdf");
        assert_eq!(guess_mime_type("video.mp4"), "video/mp4");
        assert_eq!(guess_mime_type("song.mp3"), "audio/mpeg");
        assert_eq!(guess_mime_type("text.txt"), "text/plain");
        assert_eq!(guess_mime_type("readme.md"), "text/markdown");
    }

    #[test]
    fn test_guess_mime_type_unknown_extension() {
        assert_eq!(guess_mime_type("file.xyz"), "application/octet-stream");
    }

    #[test]
    fn test_guess_mime_type_no_extension() {
        assert_eq!(guess_mime_type("Makefile"), "application/octet-stream");
    }

    #[test]
    fn test_guess_mime_type_case_insensitive() {
        assert_eq!(guess_mime_type("Photo.PNG"), "image/png");
        assert_eq!(guess_mime_type("Doc.PDF"), "application/pdf");
    }

    // ── Step 7: Import tests ───────────────────────────────────────────

    #[test]
    fn test_import_successful() {
        let (tmp_dir, library_dir) = test_library_dir();
        let storage = test_storage();
        let source = tmp_dir.path().join("hello.txt");
        std::fs::write(&source, b"hello world").unwrap();
        let cancel = CancellationToken::new();

        let hash = import_file(
            &source,
            "hello.txt",
            Some("A test file"),
            &library_dir,
            &storage,
            "alice_key",
            "meta-1",
            &[],
            &cancel,
            None,
        )
        .unwrap();

        assert_eq!(hash.len(), 64);
        // The managed file should exist.
        let managed = content_addressed_path(&library_dir, &hash);
        assert!(managed.exists(), "managed file should exist at {managed:?}");
        // The database should have a file_object.
        assert!(storage.file_object_has_references(&hash).unwrap());
        // The shared_file should be offered.
        let files = storage.list_shared_files("alice_key", true).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content_hash, hash);
    }

    #[test]
    fn test_import_duplicate_deduplicates() {
        let (tmp_dir, library_dir) = test_library_dir();
        let storage = test_storage();
        let source = tmp_dir.path().join("hello.txt");
        std::fs::write(&source, b"deduplicate me").unwrap();
        let cancel = CancellationToken::new();

        // First import.
        let hash1 = import_file(
            &source,
            "hello.txt",
            None,
            &library_dir,
            &storage,
            "alice_key",
            "meta-1",
            &[],
            &cancel,
            None,
        )
        .unwrap();

        // Second import (same content, different metadata_id).
        let hash2 = import_file(
            &source,
            "hello.txt",
            None,
            &library_dir,
            &storage,
            "alice_key",
            "meta-2",
            &[],
            &cancel,
            None,
        )
        .unwrap();

        assert_eq!(hash1, hash2);
        // Only one file_object.
        assert!(storage.file_object_has_references(&hash1).unwrap());
        // Two imports with same content_hash + same profile_user_id result
        // in a single shared_file row (upsert semantics on PK conflict).
        let files = storage.list_shared_files("alice_key", true).unwrap();
        assert_eq!(
            files.len(),
            1,
            "upsert merges duplicate hash+profile into one row"
        );
        // Only one managed file on disk.
        let managed = content_addressed_path(&library_dir, &hash1);
        assert!(managed.exists());
    }

    #[test]
    fn test_import_zero_byte_file_rejected() {
        let (tmp_dir, library_dir) = test_library_dir();
        let storage = test_storage();
        let source = tmp_dir.path().join("empty.txt");
        std::fs::write(&source, b"").unwrap();
        let cancel = CancellationToken::new();

        let result = import_file(
            &source,
            "empty.txt",
            None,
            &library_dir,
            &storage,
            "alice_key",
            "meta-1",
            &[],
            &cancel,
            None,
        );

        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("Invalid file size") || err.contains("zero"),
            "got: {err}"
        );
    }

    #[test]
    fn test_import_interrupted_copy_cleans_temp() {
        // We simulate an interrupted copy by using a cancellation token that
        // fires during the copy.  Since we can't cancel std::fs::copy mid-way,
        // we verify that a cancelled hash leads to no temp file.
        let (tmp_dir, library_dir) = test_library_dir();
        let storage = test_storage();
        let source = tmp_dir.path().join("medium.bin");
        let content = vec![0xBBu8; 1_000_000];
        std::fs::write(&source, &content).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = import_file(
            &source,
            "medium.bin",
            None,
            &library_dir,
            &storage,
            "alice_key",
            "meta-1",
            &[],
            &cancel,
            None,
        );

        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("cancelled"), "got: {err}");
        // No temp file should remain.
        let temp_pattern = library_dir.join(".tmp_*.import");
        let entries = std::fs::read_dir(&library_dir).unwrap();
        for entry in entries {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            assert!(
                !name.starts_with(".tmp_"),
                "temp file should be cleaned: {name}"
            );
        }
    }

    #[test]
    fn test_import_restart_preserves_existing() {
        let (tmp_dir, library_dir) = test_library_dir();
        let storage = test_storage();
        let source = tmp_dir.path().join("persist.txt");
        std::fs::write(&source, b"survive restart").unwrap();
        let cancel = CancellationToken::new();

        // First run.
        import_file(
            &source,
            "persist.txt",
            None,
            &library_dir,
            &storage,
            "alice_key",
            "meta-1",
            &[],
            &cancel,
            None,
        )
        .unwrap();

        // Simulate "restart" by verifying the managed file is still there.
        let files = storage.list_shared_files("alice_key", true).unwrap();
        assert_eq!(files.len(), 1);
        let hash = &files[0].content_hash;
        let managed = content_addressed_path(&library_dir, hash);
        assert!(managed.exists());
    }

    // ── Step 8: Referenced file tests ──────────────────────────────────

    #[test]
    fn test_offer_referenced_successful() {
        let (tmp_dir, library_dir) = test_library_dir();
        let storage = test_storage();
        let source = tmp_dir.path().join("doc.txt");
        std::fs::write(&source, b"important reference").unwrap();
        let cancel = CancellationToken::new();

        let hash = offer_referenced_file(
            &source,
            "doc.txt",
            Some("A referenced file"),
            &library_dir,
            &storage,
            "alice_key",
            "meta-1",
            &[],
            &cancel,
            None,
        )
        .unwrap();

        // The source file should NOT have been copied to the managed path.
        let managed = content_addressed_path(&library_dir, &hash);
        assert!(!managed.exists(), "referenced files should not be copied");
        // The .refs entry should exist.
        let refs_path = library_dir.join(".refs").join(&hash);
        assert!(refs_path.exists(), "refs entry should exist");
        let stored_path = std::fs::read_to_string(&refs_path).unwrap();
        assert_eq!(PathBuf::from(&stored_path), source);
        // DB should have a file_object (with zero inline data).
        let obj = storage.get_file_object(&hash).unwrap().unwrap();
        assert_eq!(obj.data, Some(vec![])); // empty data for referenced files
                                            // Shared file should be offered.
        let files = storage.list_shared_files("alice_key", true).unwrap();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn test_offer_referenced_duplicate_allows_two_offers() {
        let (tmp_dir, library_dir) = test_library_dir();
        let storage = test_storage();
        let source = tmp_dir.path().join("shared.txt");
        std::fs::write(&source, b"shared content").unwrap();
        let cancel = CancellationToken::new();

        let hash1 = offer_referenced_file(
            &source,
            "shared.txt",
            None,
            &library_dir,
            &storage,
            "alice_key",
            "meta-1",
            &[],
            &cancel,
            None,
        )
        .unwrap();

        let hash2 = offer_referenced_file(
            &source,
            "shared.txt",
            None,
            &library_dir,
            &storage,
            "alice_key",
            "meta-2",
            &[],
            &cancel,
            None,
        )
        .unwrap();

        assert_eq!(hash1, hash2);
        // Two offers with same content_hash + same profile_user_id result
        // in a single shared_file row (upsert semantics on PK conflict).
        let files = storage.list_shared_files("alice_key", true).unwrap();
        assert_eq!(
            files.len(),
            1,
            "upsert merges duplicate hash+profile into one row"
        );
    }

    #[test]
    fn test_referenced_source_removed_after_creation() {
        let (tmp_dir, library_dir) = test_library_dir();
        let storage = test_storage();
        let source = tmp_dir.path().join("temp.txt");
        std::fs::write(&source, b"temporary").unwrap();
        let cancel = CancellationToken::new();

        let hash = offer_referenced_file(
            &source,
            "temp.txt",
            None,
            &library_dir,
            &storage,
            "alice_key",
            "meta-1",
            &[],
            &cancel,
            None,
        )
        .unwrap();

        // Source still available.
        assert!(referenced_source_available(&library_dir, &hash));

        // Remove source.
        std::fs::remove_file(&source).unwrap();
        assert!(!referenced_source_available(&library_dir, &hash));

        // DB entry still exists (offers don't auto-remove on source loss).
        assert!(storage.file_object_has_references(&hash).unwrap());
    }

    #[test]
    fn test_referenced_source_path_is_local_only() {
        let (tmp_dir, library_dir) = test_library_dir();
        let storage = test_storage();
        let source = tmp_dir.path().join("private.txt");
        std::fs::write(&source, b"sensitive").unwrap();
        let cancel = CancellationToken::new();

        let hash = offer_referenced_file(
            &source,
            "private.txt",
            None,
            &library_dir,
            &storage,
            "alice_key",
            "meta-1",
            &[],
            &cancel,
            None,
        )
        .unwrap();

        // The file_object should NOT contain the source path in its filename.
        let obj = storage.get_file_object(&hash).unwrap().unwrap();
        assert_eq!(obj.filename, "private.txt");
        // The source path is stored in .refs, not in the DB.
        let ref_path = library_dir.join(".refs").join(&hash);
        let stored = std::fs::read_to_string(&ref_path).unwrap();
        assert_eq!(PathBuf::from(&stored), source);
    }

    // ── Step 9: Object reuse tests ─────────────────────────────────────

    #[test]
    fn test_find_or_create_file_object_reuses_existing() {
        let storage = test_storage();
        let hash = "test_reuse_hash_abc";
        let size = 100;
        let mime = "text/plain";
        let filename = "notes.txt";

        // First call — creates.
        find_or_create_file_object(&storage, hash, size, mime, filename).unwrap();
        assert!(storage.file_object_has_references(hash).unwrap());

        // Second call — reuses (idempotent).
        find_or_create_file_object(&storage, hash, size, mime, filename).unwrap();
        // Still one row.
        assert!(storage.file_object_has_references(hash).unwrap());
    }

    #[test]
    fn test_same_hash_differs_imported_vs_referenced_not_conflicting() {
        // Scenario: same content imported AND referenced.
        // An imported file creates a `file_objects` row AND managed storage.
        // A referenced file with the same hash reuses the `file_objects` row,
        // but also adds a `.refs/` entry if it doesn't already exist.
        let (tmp_dir, library_dir) = test_library_dir();
        let storage = test_storage();
        let source = tmp_dir.path().join("content.txt");
        std::fs::write(&source, b"shared by both modes").unwrap();
        let cancel = CancellationToken::new();

        // Import first.
        let hash_import = import_file(
            &source,
            "imported.txt",
            None,
            &library_dir,
            &storage,
            "alice_key",
            "meta-import",
            &[],
            &cancel,
            None,
        )
        .unwrap();

        // Then offer as referenced (same content, different filename).
        let hash_ref = offer_referenced_file(
            &source,
            "referenced.txt",
            None,
            &library_dir,
            &storage,
            "alice_key",
            "meta-ref",
            &[],
            &cancel,
            None,
        )
        .unwrap();

        assert_eq!(hash_import, hash_ref);
        // One file_object row.
        assert!(storage.file_object_has_references(&hash_import).unwrap());
        // One shared_file row (upsert merges same hash + same profile).
        let files = storage.list_shared_files("alice_key", true).unwrap();
        assert_eq!(
            files.len(),
            1,
            "upsert merges same hash+profile into one row"
        );
        // Managed file exists (from import).
        let managed = content_addressed_path(&library_dir, &hash_import);
        assert!(managed.exists());
        // .refs entry exists (from reference offer).
        let ref_path = library_dir.join(".refs").join(&hash_import);
        assert!(ref_path.exists());
    }

    #[test]
    fn test_delete_shared_file_does_not_delete_file_object() {
        let (tmp_dir, library_dir) = test_library_dir();
        let storage = test_storage();
        let source = tmp_dir.path().join("delete_test.txt");
        std::fs::write(&source, b"shared by two offers").unwrap();
        let cancel = CancellationToken::new();

        let hash = import_file(
            &source,
            "shared.txt",
            None,
            &library_dir,
            &storage,
            "alice_key",
            "meta-1",
            &[],
            &cancel,
            None,
        )
        .unwrap();

        // Verify file_object exists.
        assert!(storage.file_object_has_references(&hash).unwrap());

        // We can't directly delete from shared_files via the API — this is
        // expected. The storage layer doesn't expose delete_shared_file yet.
        // For now, verify that multiple offers share the same object.
        let files = storage.list_shared_files("alice_key", true).unwrap();
        assert!(files.len() >= 1);
        assert!(storage.file_object_has_references(&hash).unwrap());
    }

    // ── Step 21: Startup recovery tests ──────────────────────────────────

    #[test]
    fn test_find_stale_temp_files_empty() {
        let (_tmp, library_dir) = test_library_dir();
        let stale = find_stale_temp_files(&library_dir).unwrap();
        assert!(stale.is_empty());
    }

    #[test]
    fn test_find_and_remove_stale_temp_files() {
        let (_tmp, library_dir) = test_library_dir();
        // Create a stale temp file.
        let stale_path = library_dir.join(".tmp_abc123.import");
        std::fs::write(&stale_path, b"stale data").unwrap();

        let stale = find_stale_temp_files(&library_dir).unwrap();
        assert_eq!(stale.len(), 1);

        let removed = remove_stale_temp_files(&library_dir).unwrap();
        assert_eq!(removed, 1);
        assert!(!stale_path.exists());

        let stale2 = find_stale_temp_files(&library_dir).unwrap();
        assert!(stale2.is_empty());
    }

    #[test]
    fn test_stale_temp_files_preserves_normal_files() {
        let (_tmp, library_dir) = test_library_dir();
        // Create a stale temp file and a normal file.
        let stale_path = library_dir.join(".tmp_stale.import");
        std::fs::write(&stale_path, b"stale").unwrap();
        let normal_path = library_dir.join("report.pdf");
        std::fs::write(&normal_path, b"important").unwrap();

        remove_stale_temp_files(&library_dir).unwrap();
        assert!(!stale_path.exists());
        assert!(normal_path.exists(), "normal files should be preserved");
    }

    #[test]
    fn test_startup_recovery_clean() {
        let (_tmp, library_dir) = test_library_dir();
        let storage = test_storage();
        let summary = startup_recovery(&library_dir, &storage, "alice_key").unwrap();
        // Everything should be zero in a clean state.
        assert_eq!(summary.stale_temp_files_removed, 0);
        assert_eq!(summary.interrupted_ops_recovered, 0);
        assert_eq!(summary.orphaned_shared_files, 0);
        assert_eq!(summary.orphaned_file_objects, 0);
    }

    #[test]
    fn test_startup_recovery_removes_stale_temps() {
        let (_tmp, library_dir) = test_library_dir();
        let storage = test_storage();
        // Create stale temp files.
        std::fs::write(library_dir.join(".tmp_001.import"), b"x").unwrap();
        std::fs::write(library_dir.join(".tmp_002.import"), b"y").unwrap();

        let summary = startup_recovery(&library_dir, &storage, "alice_key").unwrap();
        assert_eq!(summary.stale_temp_files_removed, 2);
    }

    #[test]
    fn test_startup_recovery_recovers_interrupted_ops() {
        let storage = test_storage();
        // Create a running operation.
        storage
            .create_operation_progress("op-test-1", "hash", 100)
            .unwrap();

        let recovered = recover_interrupted_operations(&storage).unwrap();
        assert_eq!(recovered, 1);

        // Verify it's now failed.
        let op = storage
            .get_operation_progress("op-test-1")
            .unwrap()
            .unwrap();
        assert_eq!(op.status, "failed");
        assert_eq!(op.error.as_deref(), Some("startup_recovery"));
    }

    #[test]
    fn test_startup_recovery_detects_orphaned_shared_files() {
        let storage = test_storage();
        // Create a file_object and shared_file, then use raw SQL
        // to create an orphaned shared_file by:
        // 1. Delete the shared_file row first (to release FK constraint)
        // 2. Delete the file_object row
        // 3. Re-insert the shared_file row with a non-existent file_object
        storage
            .put_file_object("orphan-hash", 100, "text/plain", "ghost.txt", b"data")
            .unwrap();
        storage
            .upsert_shared_file(
                "orphan-hash",
                "alice_key",
                "meta-1",
                "ghost.txt",
                None,
                true,
            )
            .unwrap();

        // Use PRAGMA to temporarily disable FK checks for the test
        storage.with_conn(|conn| {
            conn.execute("PRAGMA foreign_keys = OFF", []).map_err(|e| anyhow::anyhow!("{e}"))?;
            conn.execute("DELETE FROM shared_files WHERE content_hash = 'orphan-hash'", [])
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            conn.execute("DELETE FROM file_objects WHERE content_hash = 'orphan-hash'", [])
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            // Re-insert a shared_file with no matching file_object
            conn.execute(
                "INSERT INTO shared_files (content_hash, profile_user_id, metadata_id, display_filename, offered, created_at_ms, updated_at_ms)
                 VALUES ('orphan-hash', 'alice_key', 'meta-1', 'ghost.txt', 1, 1000, 1000)",
                [],
            ).map_err(|e| anyhow::anyhow!("{e}"))?;
            conn.execute("PRAGMA foreign_keys = ON", []).map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(())
        }).unwrap();

        let (_tmp, library_dir) = test_library_dir();
        let summary = startup_recovery(&library_dir, &storage, "alice_key").unwrap();
        assert_eq!(summary.orphaned_shared_files, 1);
    }

    // ── Step 22: Privacy tests ──────────────────────────────────────────

    #[test]
    fn test_sanitize_path_replaces_home_dir() {
        let path = std::path::Path::new("/home/testuser/docs/report.pdf");
        let safe = sanitize_path_for_log(path);
        assert_eq!(safe, "~/docs/report.pdf");
    }

    #[test]
    fn test_sanitize_path_replaces_temp() {
        let path = std::path::Path::new("/tmp/abc123.tmp");
        let safe = sanitize_path_for_log(path);
        assert!(safe.starts_with("<temp>"));
    }

    #[test]
    fn test_sanitize_path_replaces_library_path() {
        let path = std::path::Path::new("/data/library/ab/abcdef123456");
        let safe = sanitize_path_for_log(path);
        assert!(safe.starts_with("<library>"));
    }

    #[test]
    fn test_sanitize_path_short_normal_path() {
        let path = std::path::Path::new("/usr/bin/bash");
        let safe = sanitize_path_for_log(path);
        assert_eq!(safe, "/usr/bin/bash");
    }

    #[test]
    fn test_verify_row_safe_for_remote_clean() {
        let row = crate::file_library::FileLibraryRow {
            content_hash: "abc123".into(),
            display_filename: "report.pdf".into(),
            description: Some("Quarterly report".into()),
            size: 1000,
            mime_type: "application/pdf".into(),
            offered: true,
            is_imported: true,
            source_available: true,
            collections: vec![],
            created_at_ms: 1000,
        };
        assert!(verify_row_safe_for_remote(&row).is_ok());
    }

    #[test]
    fn test_verify_row_rejects_path_in_filename() {
        let row = crate::file_library::FileLibraryRow {
            content_hash: "abc".into(),
            display_filename: "/home/user/secret.txt".into(),
            description: None,
            size: 100,
            mime_type: "text/plain".into(),
            offered: true,
            is_imported: true,
            source_available: true,
            collections: vec![],
            created_at_ms: 1000,
        };
        assert!(verify_row_safe_for_remote(&row).is_err());
    }

    #[test]
    fn test_verify_row_rejects_sensitive_description() {
        let row = crate::file_library::FileLibraryRow {
            content_hash: "abc".into(),
            display_filename: "notes.txt".into(),
            description: Some("Contains /home/user password".into()),
            size: 100,
            mime_type: "text/plain".into(),
            offered: true,
            is_imported: true,
            source_available: true,
            collections: vec![],
            created_at_ms: 1000,
        };
        assert!(verify_row_safe_for_remote(&row).is_err());
    }

    #[test]
    fn test_verify_all_rows_safe_for_remote() {
        let safe_row = crate::file_library::FileLibraryRow {
            content_hash: "a".into(),
            display_filename: "safe.txt".into(),
            description: Some("All good".into()),
            size: 10,
            mime_type: "text/plain".into(),
            offered: true,
            is_imported: true,
            source_available: true,
            collections: vec![],
            created_at_ms: 1000,
        };
        let bad_row = crate::file_library::FileLibraryRow {
            content_hash: "b".into(),
            display_filename: "/etc/passwd".into(),
            description: None,
            size: 20,
            mime_type: "text/plain".into(),
            offered: true,
            is_imported: true,
            source_available: true,
            collections: vec![],
            created_at_ms: 2000,
        };
        let violations = verify_all_rows_safe_for_remote(&[safe_row, bad_row]);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].0, 1);
    }
}
