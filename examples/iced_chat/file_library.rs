//! Local profile file library — state, filtering, sorting, and file picker workflow.
//!
//! This module manages the owner-facing "My Profile → Shared Files" view.
//! It is separate from the peer-profile overlay (which shows *remote* shared files)
//! and from the gossip-based file sharing protocol.
//!
//! The file library is backed by the SQLite `shared_files` + `file_objects` tables
//! and shows only safe metadata — no full source paths in the list view.

use std::fmt;
use std::path::PathBuf;

/// Filter for the file library list view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileLibraryFilter {
    /// All shared files (default).
    All,
    /// Files available for download by peers.
    Available,
    /// Files whose source is missing from disk.
    Missing,
    /// Files whose source has changed since import.
    Changed,
    /// Files whose sharing is disabled by profile settings.
    Disabled,
    /// Files imported into the Boru store.
    Imported,
    /// Files that reference an original file on disk.
    Referenced,
}

impl FileLibraryFilter {
    /// Human-readable label for UI display.
    pub fn label(&self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Available => "Available",
            Self::Missing => "Missing",
            Self::Changed => "Changed",
            Self::Disabled => "Disabled",
            Self::Imported => "Imported",
            Self::Referenced => "Referenced",
        }
    }

    /// All filter variants for populating a dropdown or filter bar.
    pub const ALL: &'static [FileLibraryFilter] = &[
        Self::All,
        Self::Available,
        Self::Missing,
        Self::Changed,
        Self::Disabled,
        Self::Imported,
        Self::Referenced,
    ];
}

impl fmt::Display for FileLibraryFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Sort order for the file library list view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileLibrarySort {
    /// By filename, ascending (default).
    NameAsc,
    /// By filename, descending.
    NameDesc,
    /// By file size, ascending.
    SizeAsc,
    /// By file size, descending.
    SizeDesc,
    /// By date added, newest first.
    DateAddedNewest,
    /// By date added, oldest first.
    DateAddedOldest,
}

impl FileLibrarySort {
    /// Human-readable label for UI display.
    pub fn label(&self) -> &'static str {
        match self {
            Self::NameAsc => "Name (A-Z)",
            Self::NameDesc => "Name (Z-A)",
            Self::SizeAsc => "Size (smallest)",
            Self::SizeDesc => "Size (largest)",
            Self::DateAddedNewest => "Newest",
            Self::DateAddedOldest => "Oldest",
        }
    }

    /// All sort variants for populating a dropdown.
    pub const ALL: &'static [FileLibrarySort] = &[
        Self::NameAsc,
        Self::NameDesc,
        Self::SizeAsc,
        Self::SizeDesc,
        Self::DateAddedNewest,
        Self::DateAddedOldest,
    ];

    /// Convert to storage sort key.
    pub fn storage_sort_key(&self) -> &'static str {
        match self {
            Self::NameAsc => "name_asc",
            Self::NameDesc => "name_desc",
            Self::SizeAsc => "size_asc",
            Self::SizeDesc => "size_desc",
            Self::DateAddedNewest => "newest",
            Self::DateAddedOldest => "oldest",
        }
    }
}

impl fmt::Display for FileLibrarySort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Pending operation state for feedback in the file library UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingOperation {
    /// Adding a new file to the library.
    Adding,
    /// Removing a file from the library.
    Removing(String),
    /// Updating a file's metadata.
    Updating(String),
}

/// Step in the "Add File" wizard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddFileStep {
    /// Selecting the source file from disk.
    SelectFile,
    /// Setting display name, description, storage mode, visibility, collections.
    Configure,
    /// Validation in progress.
    Validating,
    /// Validation failed — showing error.
    Error(String),
}

/// Choices for how a file is stored in the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageMode {
    /// Import file content into the Boru object store (content-addressed).
    /// The file survives deletion of the original on disk.
    Import,
    /// Reference the original file on disk. If the original is moved or deleted,
    /// the library entry becomes a broken reference.
    Reference,
}

impl StorageMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Import => "Import into Boru",
            Self::Reference => "Reference original file",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Import => {
                "File content is copied into the Boru object store. \
                 The file can be shared even if the original is deleted. \
                 Uses additional disk space."
            }
            Self::Reference => {
                "The library points to the original file on disk. \
                 No copy is made, but the file becomes unavailable if \
                 the original is moved or deleted."
            }
        }
    }
}

/// State for the "Add File" workflow wizard.
#[derive(Debug, Clone)]
pub struct AddFileState {
    /// Current step in the wizard.
    pub step: AddFileStep,
    /// Path to the selected source file.
    pub source_path: Option<PathBuf>,
    /// User-entered display name.
    pub display_name: String,
    /// User-entered description.
    pub description: String,
    /// Storage mode selected by the user.
    pub storage_mode: StorageMode,
    /// Search/filter text for assigning collections.
    pub collection_search: String,
    /// IDs of collections the file will be added to.
    pub selected_collections: Vec<i64>,
}

impl Default for AddFileState {
    fn default() -> Self {
        Self {
            step: AddFileStep::SelectFile,
            source_path: None,
            display_name: String::new(),
            description: String::new(),
            storage_mode: StorageMode::Import,
            collection_search: String::new(),
            selected_collections: Vec::new(),
        }
    }
}

/// Rich display row for a file in the library list.
///
/// Combines data from `shared_files`, `file_objects`, and computed availability.
/// Never exposes the full source path — only safe metadata.
#[derive(Debug, Clone)]
pub struct FileLibraryRow {
    /// Content hash (links to file_objects).
    pub content_hash: String,
    /// Display filename shown in the UI.
    pub display_filename: String,
    /// Optional description.
    pub description: Option<String>,
    /// File size in bytes.
    pub size: u64,
    /// MIME type.
    pub mime_type: String,
    /// Whether the file is currently offered.
    pub offered: bool,
    /// Whether the file was imported into Boru (vs referenced from disk).
    pub is_imported: bool,
    /// Whether the source file still exists on disk (only meaningful for referenced files).
    pub source_available: bool,
    /// Collection names this file belongs to.
    pub collections: Vec<String>,
    /// When the file was added (ms since UNIX epoch).
    pub created_at_ms: u64,
}

/// Owner-facing local profile file library state.
///
/// Holds the current list view state, the active filter/sort, optional
/// add-file workflow, and any pending operations or errors.
#[derive(Debug, Clone)]
pub struct LocalFileLibraryState {
    /// The list of file rows currently displayed (filtered + sorted).
    pub rows: Vec<FileLibraryRow>,
    /// All files (unfiltered, used for search/filter queries).
    pub all_rows: Vec<FileLibraryRow>,
    /// Available collections for this profile.
    pub collections: Vec<boru_chat::storage::FileCollection>,
    /// Index of the selected/highlighted file offer (None = nothing selected).
    pub selected_index: Option<usize>,
    /// Active filter.
    pub filter: FileLibraryFilter,
    /// Active sort order.
    pub sort: FileLibrarySort,
    /// Search text for filtering by name/description.
    pub search_text: String,
    /// Pending operation indicator.
    pub pending_operation: Option<PendingOperation>,
    /// Last error message (cleared on next successful operation).
    pub last_error: Option<String>,
    /// State for the "Add File" workflow (None = no active add workflow).
    pub add_file_state: Option<AddFileState>,
}

impl LocalFileLibraryState {
    /// Create empty file library state.
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            all_rows: Vec::new(),
            collections: Vec::new(),
            selected_index: None,
            filter: FileLibraryFilter::All,
            sort: FileLibrarySort::DateAddedNewest,
            search_text: String::new(),
            pending_operation: None,
            last_error: None,
            add_file_state: None,
        }
    }

    /// Apply the current filter, sort, and search text to `all_rows`
    /// and store the result in `rows`.
    pub fn apply_filter_and_sort(&mut self) {
        // Start with all rows
        let mut filtered: Vec<FileLibraryRow> = self.all_rows.clone();

        // Apply search text filter
        if !self.search_text.is_empty() {
            let q = self.search_text.to_lowercase();
            filtered.retain(|r| {
                r.display_filename.to_lowercase().contains(&q)
                    || r.description
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&q)
            });
        }

        // Apply category filter
        match self.filter {
            FileLibraryFilter::All => { /* keep all */ }
            FileLibraryFilter::Available => {
                filtered.retain(|r| r.offered && r.source_available);
            }
            FileLibraryFilter::Missing => {
                filtered.retain(|r| !r.source_available);
            }
            FileLibraryFilter::Changed => {
                // Changed detection would require comparing current file metadata
                // against stored metadata — placeholder for future enhancement.
            }
            FileLibraryFilter::Disabled => {
                filtered.retain(|r| !r.offered);
            }
            FileLibraryFilter::Imported => {
                filtered.retain(|r| r.is_imported);
            }
            FileLibraryFilter::Referenced => {
                filtered.retain(|r| !r.is_imported);
            }
        }

        // Apply sort
        match self.sort {
            FileLibrarySort::NameAsc => {
                filtered.sort_by(|a, b| a.display_filename.cmp(&b.display_filename));
            }
            FileLibrarySort::NameDesc => {
                filtered.sort_by(|a, b| b.display_filename.cmp(&a.display_filename));
            }
            FileLibrarySort::SizeAsc => {
                filtered.sort_by(|a, b| a.size.cmp(&b.size));
            }
            FileLibrarySort::SizeDesc => {
                filtered.sort_by(|a, b| b.size.cmp(&a.size));
            }
            FileLibrarySort::DateAddedNewest => {
                filtered.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
            }
            FileLibrarySort::DateAddedOldest => {
                filtered.sort_by(|a, b| a.created_at_ms.cmp(&b.created_at_ms));
            }
        }

        self.rows = filtered;
    }

    /// Clear the last error.
    pub fn clear_error(&mut self) {
        self.last_error = None;
    }
}

impl Default for LocalFileLibraryState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Step 15: Changed-file action enum ───────────────────────────────────────

/// Actions available when a referenced file has changed on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangedFileAction {
    /// Keep the old version and disable the offer (mark as stale).
    KeepOldDisabled,
    /// Update the offer to use the new file content.
    UpdateToNew,
    /// Choose a replacement file from the filesystem.
    ChooseReplacement,
    /// Remove the offer entirely.
    RemoveOffer,
}

impl ChangedFileAction {
    pub fn label(&self) -> &'static str {
        match self {
            Self::KeepOldDisabled => "Keep old (disable offer)",
            Self::UpdateToNew => "Update to new version",
            Self::ChooseReplacement => "Choose replacement file",
            Self::RemoveOffer => "Remove offer",
        }
    }
}

/// State for a changed-file notification in the UI.
#[derive(Debug, Clone)]
pub struct ChangedFileState {
    /// Content hash of the file that changed.
    pub content_hash: String,
    /// Display name of the file.
    pub display_filename: String,
    /// Original hash that was previously recorded.
    pub original_hash: String,
    /// New hash detected on disk.
    pub current_hash: String,
}

// ── Step 16: Offer removal ───────────────────────────────────────────────────

/// Options for removing a file from the profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemovalMode {
    /// Remove from profile only (tombstone the shared_file, keep file_object).
    RemoveFromProfile,
    /// Remove from profile and delete the imported copy (only if no other references).
    DeleteImportedCopy,
}

// ── Step 17: Cleanup candidate ───────────────────────────────────────────────

/// A view row for the storage management / cleanup screen.
#[derive(Debug, Clone)]
pub struct CleanupCandidateRow {
    /// Content hash prefix (first 8 chars).
    pub hash_prefix: String,
    /// Display filename.
    pub filename: String,
    /// File size in bytes.
    pub size: u64,
    /// When the file was imported.
    pub created_at_ms: u64,
    /// Whether this object is eligible for cleanup.
    pub eligible: bool,
}

/// State for the storage management / cleanup view.
#[derive(Debug, Clone)]
pub struct CleanupViewState {
    /// Candidates for cleanup.
    pub candidates: Vec<CleanupCandidateRow>,
    /// Total storage used by unreferenced imported objects.
    pub total_storage_bytes: u64,
    /// Currently running cleanup progress text.
    pub progress_text: Option<String>,
}

impl CleanupViewState {
    pub fn new() -> Self {
        Self {
            candidates: Vec::new(),
            total_storage_bytes: 0,
            progress_text: None,
        }
    }
}

// ── Step 18: Details view ────────────────────────────────────────────────────

/// Data for the details panel of a selected file offer.
#[derive(Debug, Clone)]
pub struct FileDetailData {
    /// Content hash (full, for copy).
    pub content_hash: String,
    /// Display name.
    pub display_name: String,
    /// Description.
    pub description: Option<String>,
    /// File size in bytes.
    pub size: u64,
    /// MIME type.
    pub mime_type: String,
    /// Storage mode: "Imported" or "Referenced".
    pub storage_mode: String,
    /// Availability: "Available", "Missing", "Changed", "Unverified".
    pub availability: String,
    /// When the file was last verified (None if never).
    pub last_verified: Option<u64>,
    /// Visibility (if applicable).
    pub visibility: Option<String>,
    /// Collection names this file belongs to.
    pub collections: Vec<String>,
    /// Current version / revision number.
    pub revision: u32,
    /// Whether the file is currently offered.
    pub offered: bool,
    /// Local source location (for referenced files — clearly labeled).
    pub source_location: Option<String>,
    /// Whether this file object is reused by other offers or attachments.
    pub object_reuse_info: String,
}

// ── Step 19: Pagination state ────────────────────────────────────────────────

/// Pagination state for the search/filter results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaginationState {
    /// Current page (0-indexed).
    pub page: u32,
    /// Number of items per page.
    pub page_size: u32,
    /// Total number of items across all pages.
    pub total_items: u64,
    /// Total number of pages.
    pub total_pages: u64,
}

impl PaginationState {
    pub fn new(page_size: u32) -> Self {
        Self {
            page: 0,
            page_size,
            total_items: 0,
            total_pages: 0,
        }
    }

    pub fn update_total(&mut self, total: u64) {
        self.total_items = total;
        self.total_pages = if self.page_size > 0 {
            (total + self.page_size as u64 - 1) / self.page_size as u64
        } else {
            1
        };
    }

    pub fn has_prev(&self) -> bool {
        self.page > 0
    }

    pub fn has_next(&self) -> bool {
        self.page + 1 < self.total_pages as u32
    }

    pub fn offset(&self) -> u32 {
        self.page * self.page_size
    }
}

// ── Step 20: Operation progress ──────────────────────────────────────────────

/// Status of a long-running file library operation.
#[derive(Debug, Clone)]
pub struct OperationProgress {
    /// Stable operation ID.
    pub id: String,
    /// Current stage description (e.g. "Hashing", "Importing", "Verifying").
    pub stage: String,
    /// Bytes processed so far.
    pub bytes_processed: u64,
    /// Total bytes to process.
    pub total_bytes: u64,
    /// Status: "running", "completed", "failed", "cancelled".
    pub status: String,
    /// Error message if failed.
    pub error: Option<String>,
    /// Progress percentage (0-100).
    pub percent: u32,
}

impl OperationProgress {
    pub fn new(id: String, total_bytes: u64) -> Self {
        Self {
            id,
            stage: "Starting".into(),
            bytes_processed: 0,
            total_bytes,
            status: "running".into(),
            error: None,
            percent: 0,
        }
    }

    /// Human-readable progress string, e.g. "45% — Hashing".
    pub fn display(&self) -> String {
        format!("{}% — {}", self.percent, self.stage)
    }
}

// ── File validation ────────────────────────────────────────────────────

/// Errors that can occur when validating a file for addition to the library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileValidationError {
    /// The file does not exist at the given path.
    NotFound(String),
    /// The path points to a directory, not a regular file.
    IsDirectory(String),
    /// The file is not readable (permission denied).
    NotReadable(String),
    /// The file is a symlink (rejected by policy).
    IsSymlink(String),
    /// The file size is zero or cannot be represented.
    InvalidSize(u64),
    /// The filename exceeds the maximum allowed length.
    FilenameTooLong(usize),
    /// The description exceeds the maximum allowed length.
    DescriptionTooLong(usize),
    /// Other I/O error.
    IoError(String),
}

impl std::fmt::Display for FileValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(p) => write!(f, "File not found: {p}"),
            Self::IsDirectory(p) => write!(f, "Path is a directory, not a file: {p}"),
            Self::NotReadable(p) => write!(f, "File is not readable: {p}"),
            Self::IsSymlink(p) => write!(f, "Symlinks are not allowed: {p}"),
            Self::InvalidSize(s) => write!(f, "Invalid file size: {s}"),
            Self::FilenameTooLong(n) => write!(f, "Filename exceeds maximum length ({n} chars)"),
            Self::DescriptionTooLong(n) => {
                write!(f, "Description exceeds maximum length ({n} chars)")
            }
            Self::IoError(e) => write!(f, "I/O error: {e}"),
        }
    }
}

/// Maximum length for a display filename in Unicode characters.
pub const MAX_FILENAME_LENGTH: usize = 255;
/// Maximum length for a file description in Unicode characters.
pub const MAX_DESCRIPTION_LENGTH: usize = 500;

/// Errors that can occur when validating metadata changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataValidationError {
    /// Display name is empty.
    EmptyName,
    /// Display name exceeds maximum length.
    NameTooLong(usize),
    /// Description exceeds maximum length.
    DescriptionTooLong(usize),
    /// Display name contains control characters.
    NameHasControlChars,
    /// Description contains control characters.
    DescriptionHasControlChars,
    /// Visibility value is invalid.
    InvalidVisibility(String),
}

impl std::fmt::Display for MetadataValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyName => write!(f, "Display name cannot be empty"),
            Self::NameTooLong(n) => write!(f, "Display name exceeds maximum length ({n} chars)"),
            Self::DescriptionTooLong(n) => {
                write!(f, "Description exceeds maximum length ({n} chars)")
            }
            Self::NameHasControlChars => write!(f, "Display name contains control characters"),
            Self::DescriptionHasControlChars => {
                write!(f, "Description contains control characters")
            }
            Self::InvalidVisibility(v) => write!(f, "Invalid visibility value: {v}"),
        }
    }
}

/// Validate metadata fields for a shared file offer.
///
/// Checks:
/// - Display name is not empty
/// - Display name does not exceed maximum length
/// - Description does not exceed maximum length (if provided)
/// - Display name has no control characters (U+0000-U+001F, U+007F-U+009F)
/// - Description has no control characters (if provided)
/// - Visibility is one of "public", "contacts", or "private" (if provided)
///
/// Returns `Ok(())` on success.
pub fn validate_offer_metadata(
    display_name: &str,
    description: Option<&str>,
    visibility: Option<&str>,
) -> Result<(), MetadataValidationError> {
    // Empty check
    if display_name.is_empty() {
        return Err(MetadataValidationError::EmptyName);
    }

    // Length checks
    if display_name.chars().count() > MAX_FILENAME_LENGTH {
        return Err(MetadataValidationError::NameTooLong(
            display_name.chars().count(),
        ));
    }

    if let Some(desc) = description {
        if desc.chars().count() > MAX_DESCRIPTION_LENGTH {
            return Err(MetadataValidationError::DescriptionTooLong(
                desc.chars().count(),
            ));
        }
    }

    // Control character checks
    if has_control_chars(display_name) {
        return Err(MetadataValidationError::NameHasControlChars);
    }

    if let Some(desc) = description {
        if has_control_chars(desc) {
            return Err(MetadataValidationError::DescriptionHasControlChars);
        }
    }

    // Visibility check
    if let Some(vis) = visibility {
        match vis {
            "public" | "contacts" | "private" => {}
            _ => return Err(MetadataValidationError::InvalidVisibility(vis.to_string())),
        }
    }

    Ok(())
}

/// Check if a string contains control characters (U+0000-U+001F, U+007F-U+009F).
fn has_control_chars(s: &str) -> bool {
    s.chars().any(|c| {
        let code = c as u32;
        code <= 0x1F || (0x7F..=0x9F).contains(&code)
    })
}

/// Validate a file before adding it to the library.
///
/// Checks:
/// - Source file exists on disk
/// - Source file is a regular file (not a directory)
/// - Source file is readable
/// - Source file is not a symlink
/// - File size is representable and non-negative
/// - Filename is not too long
/// - Description is not too long (if provided)
///
/// Returns `Ok(metadata)` on success, or the first validation error.
pub fn validate_file_for_library(
    path: &std::path::Path,
    display_name: &str,
    description: Option<&str>,
) -> std::result::Result<std::fs::Metadata, FileValidationError> {
    // Check if path exists
    if !path.exists() {
        return Err(FileValidationError::NotFound(
            path.to_string_lossy().to_string(),
        ));
    }

    // Symlink check (must be before metadata() to detect the link itself)
    let symlink_meta =
        std::fs::symlink_metadata(path).map_err(|e| FileValidationError::IoError(e.to_string()))?;
    if symlink_meta.is_symlink() {
        return Err(FileValidationError::IsSymlink(
            path.to_string_lossy().to_string(),
        ));
    }

    // Regular file check
    let metadata =
        std::fs::metadata(path).map_err(|e| FileValidationError::IoError(e.to_string()))?;
    if metadata.is_dir() {
        return Err(FileValidationError::IsDirectory(
            path.to_string_lossy().to_string(),
        ));
    }

    // Readable check (try to open for read)
    std::fs::File::open(path).map_err(|e| FileValidationError::NotReadable(e.to_string()))?;

    // Size check
    let size = metadata.len();
    if size == 0 {
        return Err(FileValidationError::InvalidSize(size));
    }

    // Filename length check
    if display_name.chars().count() > MAX_FILENAME_LENGTH {
        return Err(FileValidationError::FilenameTooLong(
            display_name.chars().count(),
        ));
    }

    // Description length check
    if let Some(desc) = description {
        if desc.chars().count() > MAX_DESCRIPTION_LENGTH {
            return Err(FileValidationError::DescriptionTooLong(
                desc.chars().count(),
            ));
        }
    }

    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_labels_are_distinct() {
        let mut labels = std::collections::HashSet::new();
        for f in FileLibraryFilter::ALL {
            assert!(labels.insert(f.label()), "duplicate label: {}", f.label());
        }
    }

    #[test]
    fn test_sort_labels_are_distinct() {
        let mut labels = std::collections::HashSet::new();
        for s in FileLibrarySort::ALL {
            assert!(labels.insert(s.label()), "duplicate label: {}", s.label());
        }
    }

    #[test]
    fn test_storage_mode_descriptions_non_empty() {
        assert!(!StorageMode::Import.description().is_empty());
        assert!(!StorageMode::Reference.description().is_empty());
    }

    #[test]
    fn test_apply_filter_all_keeps_everything() {
        let mut state = LocalFileLibraryState::new();
        state.all_rows = vec![
            FileLibraryRow {
                content_hash: "a".into(),
                display_filename: "a.txt".into(),
                description: None,
                size: 10,
                mime_type: "text/plain".into(),
                offered: true,
                is_imported: true,
                source_available: true,
                collections: vec![],
                created_at_ms: 1000,
            },
            FileLibraryRow {
                content_hash: "b".into(),
                display_filename: "b.txt".into(),
                description: None,
                size: 20,
                mime_type: "text/plain".into(),
                offered: false,
                is_imported: false,
                source_available: true,
                collections: vec![],
                created_at_ms: 2000,
            },
        ];
        state.filter = FileLibraryFilter::All;
        state.apply_filter_and_sort();
        assert_eq!(state.rows.len(), 2);
    }

    #[test]
    fn test_apply_filter_available() {
        let mut state = LocalFileLibraryState::new();
        state.all_rows = vec![
            FileLibraryRow {
                content_hash: "a".into(),
                display_filename: "a.txt".into(),
                description: None,
                size: 10,
                mime_type: "text/plain".into(),
                offered: true,
                is_imported: true,
                source_available: true,
                collections: vec![],
                created_at_ms: 1000,
            },
            FileLibraryRow {
                content_hash: "b".into(),
                display_filename: "b.txt".into(),
                description: None,
                size: 20,
                mime_type: "text/plain".into(),
                offered: true,
                is_imported: false,
                source_available: false,
                collections: vec![],
                created_at_ms: 2000,
            },
        ];
        state.filter = FileLibraryFilter::Available;
        state.apply_filter_and_sort();
        assert_eq!(state.rows.len(), 1);
        assert_eq!(state.rows[0].content_hash, "a");
    }

    #[test]
    fn test_apply_filter_missing() {
        let mut state = LocalFileLibraryState::new();
        state.all_rows = vec![
            FileLibraryRow {
                content_hash: "a".into(),
                display_filename: "a.txt".into(),
                description: None,
                size: 10,
                mime_type: "text/plain".into(),
                offered: true,
                is_imported: false,
                source_available: false,
                collections: vec![],
                created_at_ms: 1000,
            },
            FileLibraryRow {
                content_hash: "b".into(),
                display_filename: "b.txt".into(),
                description: None,
                size: 20,
                mime_type: "text/plain".into(),
                offered: true,
                is_imported: false,
                source_available: true,
                collections: vec![],
                created_at_ms: 2000,
            },
        ];
        state.filter = FileLibraryFilter::Missing;
        state.apply_filter_and_sort();
        assert_eq!(state.rows.len(), 1);
        assert_eq!(state.rows[0].content_hash, "a");
    }

    #[test]
    fn test_apply_filter_imported_vs_referenced() {
        let mut state = LocalFileLibraryState::new();
        state.all_rows = vec![
            FileLibraryRow {
                content_hash: "a".into(),
                display_filename: "imported.txt".into(),
                description: None,
                size: 10,
                mime_type: "text/plain".into(),
                offered: true,
                is_imported: true,
                source_available: true,
                collections: vec![],
                created_at_ms: 1000,
            },
            FileLibraryRow {
                content_hash: "b".into(),
                display_filename: "referenced.txt".into(),
                description: None,
                size: 20,
                mime_type: "text/plain".into(),
                offered: true,
                is_imported: false,
                source_available: true,
                collections: vec![],
                created_at_ms: 2000,
            },
        ];

        state.filter = FileLibraryFilter::Imported;
        state.apply_filter_and_sort();
        assert_eq!(state.rows.len(), 1);
        assert_eq!(state.rows[0].content_hash, "a");

        state.filter = FileLibraryFilter::Referenced;
        state.apply_filter_and_sort();
        assert_eq!(state.rows.len(), 1);
        assert_eq!(state.rows[0].content_hash, "b");
    }

    #[test]
    fn test_filter_disabled() {
        let mut state = LocalFileLibraryState::new();
        state.all_rows = vec![
            FileLibraryRow {
                content_hash: "a".into(),
                display_filename: "offered.txt".into(),
                description: None,
                size: 10,
                mime_type: "text/plain".into(),
                offered: true,
                is_imported: true,
                source_available: true,
                collections: vec![],
                created_at_ms: 1000,
            },
            FileLibraryRow {
                content_hash: "b".into(),
                display_filename: "disabled.txt".into(),
                description: None,
                size: 20,
                mime_type: "text/plain".into(),
                offered: false,
                is_imported: false,
                source_available: true,
                collections: vec![],
                created_at_ms: 2000,
            },
        ];
        state.filter = FileLibraryFilter::Disabled;
        state.apply_filter_and_sort();
        assert_eq!(state.rows.len(), 1);
        assert_eq!(state.rows[0].content_hash, "b");
    }

    #[test]
    fn test_sort_by_name_asc() {
        let mut state = LocalFileLibraryState::new();
        state.all_rows = vec![
            FileLibraryRow {
                content_hash: "b".into(),
                display_filename: "z.txt".into(),
                description: None,
                size: 10,
                mime_type: "text/plain".into(),
                offered: true,
                is_imported: true,
                source_available: true,
                collections: vec![],
                created_at_ms: 1000,
            },
            FileLibraryRow {
                content_hash: "a".into(),
                display_filename: "a.txt".into(),
                description: None,
                size: 20,
                mime_type: "text/plain".into(),
                offered: true,
                is_imported: true,
                source_available: true,
                collections: vec![],
                created_at_ms: 2000,
            },
        ];
        state.sort = FileLibrarySort::NameAsc;
        state.apply_filter_and_sort();
        assert_eq!(state.rows[0].content_hash, "a");
        assert_eq!(state.rows[1].content_hash, "b");
    }

    #[test]
    fn test_sort_by_size_desc() {
        let mut state = LocalFileLibraryState::new();
        state.all_rows = vec![
            FileLibraryRow {
                content_hash: "small".into(),
                display_filename: "small.txt".into(),
                description: None,
                size: 10,
                mime_type: "text/plain".into(),
                offered: true,
                is_imported: true,
                source_available: true,
                collections: vec![],
                created_at_ms: 1000,
            },
            FileLibraryRow {
                content_hash: "large".into(),
                display_filename: "large.txt".into(),
                description: None,
                size: 100,
                mime_type: "text/plain".into(),
                offered: true,
                is_imported: true,
                source_available: true,
                collections: vec![],
                created_at_ms: 2000,
            },
        ];
        state.sort = FileLibrarySort::SizeDesc;
        state.apply_filter_and_sort();
        assert_eq!(state.rows[0].content_hash, "large");
        assert_eq!(state.rows[1].content_hash, "small");
    }

    #[test]
    fn test_search_filter() {
        let mut state = LocalFileLibraryState::new();
        state.all_rows = vec![
            FileLibraryRow {
                content_hash: "a".into(),
                display_filename: "report.pdf".into(),
                description: Some("Monthly report".into()),
                size: 100,
                mime_type: "application/pdf".into(),
                offered: true,
                is_imported: true,
                source_available: true,
                collections: vec![],
                created_at_ms: 1000,
            },
            FileLibraryRow {
                content_hash: "b".into(),
                display_filename: "photo.jpg".into(),
                description: Some("Vacation photo".into()),
                size: 200,
                mime_type: "image/jpeg".into(),
                offered: true,
                is_imported: true,
                source_available: true,
                collections: vec![],
                created_at_ms: 2000,
            },
        ];
        state.search_text = "report".into();
        state.apply_filter_and_sort();
        assert_eq!(state.rows.len(), 1);
        assert_eq!(state.rows[0].content_hash, "a");
    }

    #[test]
    fn test_search_by_description() {
        let mut state = LocalFileLibraryState::new();
        state.all_rows = vec![FileLibraryRow {
            content_hash: "a".into(),
            display_filename: "notes.txt".into(),
            description: Some("work notes".into()),
            size: 10,
            mime_type: "text/plain".into(),
            offered: true,
            is_imported: true,
            source_available: true,
            collections: vec![],
            created_at_ms: 1000,
        }];
        state.search_text = "work".into();
        state.apply_filter_and_sort();
        assert_eq!(state.rows.len(), 1);
    }

    #[test]
    fn test_clear_error() {
        let mut state = LocalFileLibraryState::new();
        state.last_error = Some("something went wrong".into());
        state.clear_error();
        assert!(state.last_error.is_none());
    }

    // ── File validation tests ──

    #[test]
    fn test_validate_nonexistent_file() {
        let path = std::path::Path::new("/tmp/__nonexistent_file_for_test__");
        let result = validate_file_for_library(path, "test.txt", None);
        assert!(matches!(result, Err(FileValidationError::NotFound(_))));
    }

    #[test]
    fn test_validate_directory_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let result = validate_file_for_library(dir.path(), "test.txt", None);
        assert!(matches!(result, Err(FileValidationError::IsDirectory(_))));
    }

    #[test]
    fn test_validate_symlink_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let real_file = dir.path().join("real.txt");
        std::fs::write(&real_file, "hello").unwrap();
        let link = dir.path().join("link.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_file, &link).unwrap();
        let result = validate_file_for_library(&link, "link.txt", None);
        assert!(matches!(result, Err(FileValidationError::IsSymlink(_))));
    }

    #[test]
    fn test_validate_zero_byte_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.txt");
        std::fs::write(&path, b"").unwrap();
        let result = validate_file_for_library(&path, "empty.txt", None);
        assert!(matches!(result, Err(FileValidationError::InvalidSize(0))));
    }

    #[test]
    fn test_validate_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("valid.txt");
        std::fs::write(&path, b"hello world").unwrap();
        let result = validate_file_for_library(&path, "valid.txt", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_filename_too_long() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("valid.txt");
        std::fs::write(&path, b"hello").unwrap();
        let long_name = "a".repeat(MAX_FILENAME_LENGTH + 1);
        let result = validate_file_for_library(&path, &long_name, None);
        assert!(matches!(
            result,
            Err(FileValidationError::FilenameTooLong(_))
        ));
    }

    #[test]
    fn test_validate_description_too_long() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("valid.txt");
        std::fs::write(&path, b"hello").unwrap();
        let long_desc = "a".repeat(MAX_DESCRIPTION_LENGTH + 1);
        let result = validate_file_for_library(&path, "valid.txt", Some(&long_desc));
        assert!(matches!(
            result,
            Err(FileValidationError::DescriptionTooLong(_))
        ));
    }

    #[test]
    fn test_validate_valid_with_description() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("valid.txt");
        std::fs::write(&path, b"hello world").unwrap();
        let result = validate_file_for_library(&path, "valid.txt", Some("A test file"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_unusual_unicode_filename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("valid.txt");
        std::fs::write(&path, b"hello").unwrap();
        // Unicode filename with emoji
        let result = validate_file_for_library(&path, "📁 report-2024.txt", None);
        assert!(result.is_ok());
    }

    // ── Step 10: Metadata validation tests ────────────────────────────

    #[test]
    fn test_validate_metadata_empty_name_rejected() {
        let result = validate_offer_metadata("", None, None);
        assert!(matches!(result, Err(MetadataValidationError::EmptyName)));
    }

    #[test]
    fn test_validate_metadata_valid_name_accepted() {
        let result =
            validate_offer_metadata("report.pdf", Some("Quarterly report"), Some("public"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_metadata_name_too_long() {
        let long_name = "a".repeat(MAX_FILENAME_LENGTH + 1);
        let result = validate_offer_metadata(&long_name, None, None);
        assert!(matches!(
            result,
            Err(MetadataValidationError::NameTooLong(_))
        ));
    }

    #[test]
    fn test_validate_metadata_description_too_long() {
        let long_desc = "a".repeat(MAX_DESCRIPTION_LENGTH + 1);
        let result = validate_offer_metadata("file.txt", Some(&long_desc), None);
        assert!(matches!(
            result,
            Err(MetadataValidationError::DescriptionTooLong(_))
        ));
    }

    #[test]
    fn test_validate_metadata_control_chars_in_name() {
        let result = validate_offer_metadata("file\x00name", None, None);
        assert!(matches!(
            result,
            Err(MetadataValidationError::NameHasControlChars)
        ));
    }

    #[test]
    fn test_validate_metadata_control_chars_in_description() {
        let result = validate_offer_metadata("file.txt", Some("desc\x00with null"), None);
        assert!(matches!(
            result,
            Err(MetadataValidationError::DescriptionHasControlChars)
        ));
    }

    #[test]
    fn test_validate_metadata_invalid_visibility() {
        let result = validate_offer_metadata("file.txt", None, Some("secret"));
        assert!(matches!(
            result,
            Err(MetadataValidationError::InvalidVisibility(_))
        ));
    }

    #[test]
    fn test_validate_metadata_valid_visibility_values() {
        assert!(validate_offer_metadata("file.txt", None, Some("public")).is_ok());
        assert!(validate_offer_metadata("file.txt", None, Some("contacts")).is_ok());
        assert!(validate_offer_metadata("file.txt", None, Some("private")).is_ok());
    }

    #[test]
    fn test_validate_metadata_unicode_name_accepted() {
        let result = validate_offer_metadata("résumé.pdf", Some("Mon CV"), None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_metadata_no_visibility_defaults_to_ok() {
        let result = validate_offer_metadata("file.txt", None, None);
        assert!(result.is_ok());
    }
}
