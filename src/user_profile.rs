//! User profile and shared file data models for boru-chat.
//!
//! This module defines [`UserProfile`] (local user identity and preferences)
//! and [`SharedFile`] (metadata about files the user shares with peers).
//!
//! The on-disk JSON file `profile.json` lives beside `secret_key.txt` in the
//! user's data directory alongside `friends.json` and `conversations.json`.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use iroh::PublicKey;
use n0_error::{Result, StdResultExt, bail_any};
use serde::{Deserialize, Deserializer, Serialize};

use crate::chat_core::atomic_write::atomic_write_json;

// ── Constants ────────────────────────────────────────────────────────────

/// Current schema version for `profile.json`.
const SCHEMA_VERSION: u32 = 1;

/// Name of the on-disk profile file (lives beside `secret_key.txt`).
pub const PROFILE_FILE_NAME: &str = "profile.json";

/// Maximum display name length in Unicode characters.
const MAX_DISPLAY_NAME_LENGTH: usize = 64;

/// Maximum bio length in Unicode characters.
const MAX_BIO_LENGTH: usize = 140;

/// Default maximum file size in bytes (100 MB).
const DEFAULT_MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

fn default_max_file_size() -> u64 {
    DEFAULT_MAX_FILE_SIZE
}

fn profile_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PROFILE_FILE_NAME)
}

/// Determine the home directory for default path resolution.
fn home_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// Default shared folder path, by platform.
///
/// - Linux / macOS: `~/Documents/Boru/Shared`
/// - Windows:       `~\Documents\Boru\Shared`
fn default_shared_folder_path() -> PathBuf {
    home_dir().join("Documents").join("Boru").join("Shared")
}

// ── UserProfile ──────────────────────────────────────────────────────────

/// Local user identity and file-sharing preferences.
///
/// Persisted as part of `profile.json`.  The `user_id` is set to the local
/// node's [`PublicKey`] and should not change across restarts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserProfile {
    /// The local node's iroh public key (node identity).
    pub user_id: PublicKey,

    /// Human-readable display name (max 64 characters).
    #[serde(default)]
    pub display_name: String,

    /// Short biography (max 140 characters, enforced at struct level).
    #[serde(default)]
    pub bio: String,

    /// Reference to an image stored in [`ImageStore`](crate::image_store::ImageStore).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_identifier: Option<String>,

    /// Default path for shared files.
    #[serde(default = "default_shared_folder_path")]
    pub shared_folder_path: PathBuf,

    /// Whether file sharing is enabled.
    #[serde(default)]
    pub file_sharing_enabled: bool,

    /// Whether other peers are allowed to download shared files.
    #[serde(default)]
    pub allow_downloads: bool,

    /// Maximum size in bytes for incoming files.
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,

    /// Allowed file extensions for incoming files (empty = all allowed).
    #[serde(default)]
    pub allowed_extensions: Vec<String>,
}

impl Default for UserProfile {
    fn default() -> Self {
        // A 32-byte all-zeros key is valid for ed25519-based PublicKey and
        // serves as a sentinel placeholder until the real local identity is
        // assigned on first load.
        let placeholder =
            PublicKey::from_bytes(&[0u8; 32]).expect("32 zero bytes is a valid ed25519 public key");
        Self {
            user_id: placeholder,
            display_name: String::new(),
            bio: String::new(),
            avatar_identifier: None,
            shared_folder_path: default_shared_folder_path(),
            file_sharing_enabled: false,
            allow_downloads: false,
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            allowed_extensions: Vec::new(),
        }
    }
}

impl UserProfile {
    /// Create a new profile with the given local public key and empty fields.
    pub fn new(user_id: PublicKey) -> Self {
        Self {
            user_id,
            ..Self::default()
        }
    }

    /// Check whether a file with the given size and extension may be
    /// announced (published in a `ProfileUpdate`).
    ///
    /// Returns `Ok(())` if the file passes all filters, or `Err` with a
    /// human-readable reason string describing the first violation.
    ///
    /// Checks performed:
    /// - `file_sharing_enabled` must be `true`
    /// - File size must be ≤ `max_file_size`
    /// - If `allowed_extensions` is non-empty, the extension must be in it
    pub fn is_file_announce_allowed(
        &self,
        size: u64,
        extension: &str,
    ) -> std::result::Result<(), String> {
        if !self.file_sharing_enabled {
            return Err("File sharing is disabled".into());
        }
        if size > self.max_file_size {
            return Err(format!(
                "File size ({} bytes) exceeds the maximum ({})",
                size, self.max_file_size
            ));
        }
        if !self.allowed_extensions.is_empty() {
            let ext = extension.trim().trim_start_matches('.').to_lowercase();
            let allowed: Vec<&str> = self.allowed_extensions.iter().map(|s| s.trim()).collect();
            if !allowed.contains(&ext.as_str()) {
                return Err(format!(
                    "File extension '.{ext}' is not in the allowed list: {}",
                    self.allowed_extensions.join(", ")
                ));
            }
        }
        Ok(())
    }

    /// Validate that `path` is contained within `root` (after canonicalization).
    ///
    /// Returns `true` if the canonical path of `path` starts with the
    /// canonical path of `root`, meaning the file is inside the shared folder.
    pub fn is_path_contained(path: &Path, root: &Path) -> bool {
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => return false,
        };
        let root_canonical = match root.canonicalize() {
            Ok(r) => r,
            Err(_) => return false,
        };
        canonical.starts_with(&root_canonical)
    }

    /// Check that a symlink at `path` does not escape the shared `root` folder.
    ///
    /// If `path` is not a symlink, this returns `true` (allowed by default).
    /// If it *is* a symlink, we resolve its target and check containment.
    pub fn symlink_is_safe(path: &Path, root: &Path) -> bool {
        let meta = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(_) => return false,
        };
        if !meta.is_symlink() {
            return true; // not a symlink — nothing to escape
        }
        let target = match std::fs::read_link(path) {
            Ok(t) => t,
            Err(_) => return false,
        };
        let resolved = if target.is_absolute() {
            target
        } else {
            // Relative target: resolve against the symlink's parent directory
            path.parent().unwrap_or(Path::new(".")).join(&target)
        };
        Self::is_path_contained(&resolved, root)
    }

    /// Validate a filename received from a peer.
    ///
    /// Rejects filenames that contain path separators (preventing path
    /// traversal on the receiving side).
    pub fn validate_received_filename(filename: &str) -> std::result::Result<(), String> {
        if filename.is_empty() {
            return Err("Received empty filename".into());
        }
        if filename.contains('/') || filename.contains('\\') {
            return Err(format!(
                "Received filename contains path separator: {filename:?}"
            ));
        }
        if filename == "." || filename == ".." {
            return Err(format!(
                "Received filename is a directory reference: {filename:?}"
            ));
        }
        Ok(())
    }

    /// File name for the on-disk profile JSON.
    pub const FILE_NAME: &'static str = PROFILE_FILE_NAME;

    /// Convenience alias for `file_sharing_enabled` used by GUI code.
    pub fn shared_folder_enabled(&self) -> bool {
        self.file_sharing_enabled
    }

    /// Return the shared folder path.
    pub fn shared_folder_path(&self) -> &Path {
        &self.shared_folder_path
    }

    /// Set shared_folder_enabled (alias for file_sharing_enabled).
    pub fn set_shared_folder_enabled(&mut self, enabled: bool) {
        self.file_sharing_enabled = enabled;
    }

    /// Returns `true` if file sharing is globally enabled.
    ///
    /// This is the canonical check that consumers should call before
    /// attempting to announce or transfer files.
    pub fn is_sharing_enabled(&self) -> bool {
        self.file_sharing_enabled
    }

    /// Returns `true` if other peers are allowed to download our shared files.
    pub fn is_download_allowed(&self) -> bool {
        self.allow_downloads
    }

    /// Check whether a file at `path` is allowed by the current profile
    /// sharing constraints (size ≤ `max_file_size` and extension in the
    /// allowed list, or no list = all allowed).
    ///
    /// Reads the file's metadata from disk (size only — contents are not read).
    /// Returns `Ok(())` if the file passes all checks, or `Err` with a
    /// human-readable reason on the first violation.
    pub fn is_file_allowed(&self, path: &Path) -> std::result::Result<(), String> {
        let metadata = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => return Err(format!("Cannot read file metadata: {e}")),
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_lowercase();
        self.is_file_announce_allowed(metadata.len(), &ext)
    }

    /// Convenience: load the profile from a data directory, extracting just
    /// the [`UserProfile`] from the [`UserProfileStore`].
    pub fn load(data_dir: impl AsRef<Path>, local_public: PublicKey) -> Result<Self> {
        let store = UserProfileStore::load(data_dir, local_public)?;
        Ok(store.profile)
    }

    /// Convenience: save this profile by wrapping it in a [`UserProfileStore`]
    /// and persisting it atomically.
    pub fn save(&self, data_dir: impl AsRef<Path>) -> Result<PathBuf> {
        let mut store = UserProfileStore::empty_at(data_dir.as_ref(), self.user_id);
        store.set_profile(self.clone());
        store.save()
    }

    /// Validate profile fields, returning an error on constraint violation.
    ///
    /// Checks:
    /// - `display_name` must be at most [`MAX_DISPLAY_NAME_LENGTH`] characters.
    /// - `bio` must be at most [`MAX_BIO_LENGTH`] characters.
    pub fn validate(&self) -> Result<()> {
        if self.display_name.chars().count() > MAX_DISPLAY_NAME_LENGTH {
            bail_any!(
                "display_name exceeds maximum length of {} characters (got {})",
                MAX_DISPLAY_NAME_LENGTH,
                self.display_name.chars().count()
            );
        }
        if self.bio.chars().count() > MAX_BIO_LENGTH {
            bail_any!(
                "bio exceeds maximum length of {} characters (got {})",
                MAX_BIO_LENGTH,
                self.bio.chars().count()
            );
        }
        Ok(())
    }
}

// ── SharedFile ───────────────────────────────────────────────────────────

/// Metadata about a file the user shares with peers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SharedFile {
    /// Unique identifier (hash of filename + size + modified_time).
    pub id: String,

    /// Original filename.
    pub filename: String,

    /// Absolute local path of the file inside the shared folder.
    #[serde(default)]
    pub path: PathBuf,

    /// File size in bytes.
    pub size: u64,

    /// MIME type of the file.
    pub mime_type: String,

    /// Last modification time (seconds since UNIX_EPOCH).
    #[serde(
        serialize_with = "serialize_systemtime",
        deserialize_with = "deserialize_systemtime"
    )]
    pub modified_time: SystemTime,

    /// Content hash — `None` until lazy-hashing completes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<[u8; 32]>,

    /// Blob reference — `None` until the file has been uploaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<iroh_blobs::Hash>,

    /// If `true`, this file exceeds the profile's `max_file_size` and
    /// should NOT be published in ProfileUpdate (but remains visible locally).
    #[serde(default, skip_serializing_if = "is_false")]
    pub over_limit: bool,

    /// If `true`, this file's extension is not in the profile's
    /// `allowed_extensions` list and should NOT be announced.
    #[serde(default, skip_serializing_if = "is_false")]
    pub extension_blocked: bool,
}

fn is_false(b: &bool) -> bool {
    !b
}

impl SharedFile {
    /// Create a new shared file entry, deriving `id` from filename, size, and
    /// modified time via blake3.
    pub fn new(
        filename: impl Into<String>,
        size: u64,
        mime_type: impl Into<String>,
        modified_time: SystemTime,
    ) -> Self {
        let filename = filename.into();
        let mime_type = mime_type.into();
        let id = compute_shared_file_id(&filename, size, modified_time);
        Self {
            id,
            filename,
            path: PathBuf::new(),
            size,
            mime_type,
            modified_time,
            hash: None,
            blob_id: None,
            over_limit: false,
            extension_blocked: false,
        }
    }

    /// Returns `true` if this file should be announced in a ProfileUpdate.
    /// Files that are over the size limit or have a blocked extension are
    /// kept in the local index but not published to peers.
    pub fn is_announceable(&self) -> bool {
        !self.over_limit && !self.extension_blocked
    }
}

/// Compute a stable identifier for a shared file from its metadata.
fn compute_shared_file_id(filename: &str, size: u64, modified_time: SystemTime) -> String {
    let ts = modified_time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut hasher = blake3::Hasher::new();
    hasher.update(filename.as_bytes());
    hasher.update(&size.to_le_bytes());
    hasher.update(&ts.to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

// ── SystemTime serde helpers ────────────────────────────────────────────

fn serialize_systemtime<S>(time: &SystemTime, serializer: S) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let secs = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    serde::Serialize::serialize(&secs, serializer)
}

fn deserialize_systemtime<'de, D>(deserializer: D) -> std::result::Result<SystemTime, D::Error>
where
    D: Deserializer<'de>,
{
    let secs: u64 = Deserialize::deserialize(deserializer)?;
    Ok(UNIX_EPOCH + std::time::Duration::from_secs(secs))
}

// ── Custom deserializer for Vec<SharedFile> (skip corrupt entries) ──────

/// Deserialize a `Vec<SharedFile>`, silently skipping entries that fail to
/// parse (logging via eprintln).  This keeps the store loadable even if
/// individual file entries become corrupt.
fn deserialize_shared_files<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<SharedFile>, D::Error>
where
    D: Deserializer<'de>,
{
    // Deserialize as a vec of raw JSON values first
    let values: Vec<serde_json::Value> = Vec::deserialize(deserializer)?;
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        match serde_json::from_value::<SharedFile>(value) {
            Ok(file) => result.push(file),
            Err(err) => {
                eprintln!("warning: skipping corrupt shared file entry: {err}");
            }
        }
    }
    Ok(result)
}

// ── UserProfileStore ─────────────────────────────────────────────────────

/// Persistent user profile and shared file metadata store.
///
/// Serialised to `profile.json` in the configured data directory.
#[derive(Debug, Serialize, Deserialize)]
pub struct UserProfileStore {
    /// Format version for future migrations.
    #[serde(default = "default_schema_version")]
    schema_version: u32,

    /// The user's profile.
    profile: UserProfile,

    /// Metadata about files the user shares.
    #[serde(default, deserialize_with = "deserialize_shared_files")]
    shared_files: Vec<SharedFile>,

    /// Data directory used for load/save operations (not serialised).
    #[serde(skip)]
    data_dir: PathBuf,
}

impl Default for UserProfileStore {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            profile: UserProfile::default(),
            shared_files: Vec::new(),
            data_dir: PathBuf::new(),
        }
    }
}

impl UserProfileStore {
    /// Construct an empty store bound to a data directory, with a default
    /// profile using the given local public key.
    pub fn empty_at(data_dir: impl Into<PathBuf>, local_public: PublicKey) -> Self {
        Self {
            profile: UserProfile::new(local_public),
            data_dir: data_dir.into(),
            ..Self::default()
        }
    }

    /// Return the data directory used by this store.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Return the on-disk profile file path.
    pub fn file_path(&self) -> PathBuf {
        profile_file_path(&self.data_dir)
    }

    /// Return a reference to the current profile.
    pub fn profile(&self) -> &UserProfile {
        &self.profile
    }

    /// Return a mutable reference to the current profile.
    pub fn profile_mut(&mut self) -> &mut UserProfile {
        &mut self.profile
    }

    /// Replace the current profile with a new one.
    pub fn set_profile(&mut self, profile: UserProfile) {
        self.profile = profile;
    }

    /// Return an immutable iterator over shared files.
    pub fn shared_files(&self) -> &[SharedFile] {
        &self.shared_files
    }

    /// Return a mutable reference to the shared files list.
    pub fn shared_files_mut(&mut self) -> &mut Vec<SharedFile> {
        &mut self.shared_files
    }

    /// Add a shared file entry.
    pub fn add_shared_file(&mut self, file: SharedFile) {
        self.shared_files.push(file);
    }

    /// Remove a shared file by id.  Returns `true` if an entry was removed.
    pub fn remove_shared_file(&mut self, id: &str) -> bool {
        let before = self.shared_files.len();
        self.shared_files.retain(|f| f.id != id);
        self.shared_files.len() < before
    }

    /// Load the profile store from disk.
    ///
    /// If `profile.json` does not exist, a new store is created with the
    /// given local public key and default values.  Corrupt JSON or an
    /// invalid schema version returns an error so callers can decide on
    /// recovery strategy.
    pub fn load(data_dir: impl AsRef<Path>, local_public: PublicKey) -> Result<Self> {
        let data_dir = data_dir.as_ref();
        let path = profile_file_path(data_dir);
        if !path.exists() {
            return Ok(Self::empty_at(data_dir, local_public));
        }

        let raw = fs::read_to_string(&path)
            .with_std_context(|_| format!("failed to read profile file {}", path.display()))?;
        let mut store: Self = serde_json::from_str(&raw)
            .with_std_context(|_| format!("failed to parse profile file {}", path.display()))?;

        if !(1..=SCHEMA_VERSION).contains(&store.schema_version) {
            return Err(n0_error::anyerr!(
                "unsupported profile schema version {} in {}",
                store.schema_version,
                path.display()
            ));
        }
        store.schema_version = SCHEMA_VERSION;

        // If the loaded profile has a placeholder user_id (all zeros, from
        // an earlier empty_at), override it with the actual local public key.
        let placeholder =
            PublicKey::from_bytes(&[0u8; 32]).expect("32 zero bytes is a valid ed25519 public key");
        if store.profile.user_id == placeholder {
            store.profile.user_id = local_public;
        }

        store.data_dir = data_dir.to_path_buf();

        // Validate the loaded profile — if it fails, bail so the caller
        // can decide what to do (e.g. fall back to an empty store).
        store.profile.validate()?;

        Ok(store)
    }

    /// Load a store, logging and falling back to an empty store on failure.
    pub fn load_or_default(data_dir: impl AsRef<Path>, local_public: PublicKey) -> Self {
        let data_dir = data_dir.as_ref();
        match Self::load(data_dir, local_public) {
            Ok(store) => store,
            Err(err) => {
                eprintln!(
                    "warning: starting with a fresh profile; failed to load {}: {err}",
                    profile_file_path(data_dir).display()
                );
                Self::empty_at(data_dir, local_public)
            }
        }
    }

    /// Persist the store atomically to `profile.json`.
    ///
    /// Validates the profile before writing.  Returns the path of the
    /// written file on success.
    pub fn save(&self) -> Result<PathBuf> {
        let data_dir = self.data_dir();
        if data_dir.as_os_str().is_empty() {
            return Err(n0_error::anyerr!(
                "profile store has no data directory bound to it",
            ));
        }
        // Validate before persisting.
        self.profile.validate()?;
        let path = self.file_path();
        atomic_write_json(&path, self, "profile store")?;
        Ok(path)
    }
}

/// Check whether a file can be announced given profile settings.
///
/// Delegates to [`UserProfile::is_file_announce_allowed`].
pub fn check_file_announce_allowed(
    profile: &UserProfile,
    size: u64,
    extension: &str,
) -> std::result::Result<(), String> {
    profile.is_file_announce_allowed(size, extension)
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Helper: create a deterministic public key for testing.
    fn test_key() -> PublicKey {
        PublicKey::from_bytes(&[1u8; 32]).expect("32 one-bytes is a valid ed25519 public key")
    }

    #[test]
    fn validate_accepts_empty_profile() {
        let profile = UserProfile::new(test_key());
        assert!(profile.validate().is_ok());
    }

    #[test]
    fn validate_rejects_overlong_display_name() {
        let mut profile = UserProfile::new(test_key());
        profile.display_name = "a".repeat(65);
        let err = profile.validate().unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("display_name"),
            "error should mention display_name: {msg}"
        );
        assert!(msg.contains("64"), "error should mention max length: {msg}");
    }

    #[test]
    fn validate_accepts_max_length_display_name() {
        let mut profile = UserProfile::new(test_key());
        profile.display_name = "a".repeat(64);
        assert!(profile.validate().is_ok());
    }

    #[test]
    fn validate_rejects_overlong_bio() {
        let mut profile = UserProfile::new(test_key());
        profile.bio = "b".repeat(141);
        let err = profile.validate().unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("bio"), "error should mention bio: {msg}");
        assert!(
            msg.contains("140"),
            "error should mention max length: {msg}"
        );
    }

    #[test]
    fn validate_accepts_max_length_bio() {
        let mut profile = UserProfile::new(test_key());
        profile.bio = "b".repeat(140);
        assert!(profile.validate().is_ok());
    }

    #[test]
    fn validate_uses_char_count_not_byte_count() {
        let mut profile = UserProfile::new(test_key());
        // 64 multi-byte (4-byte) emoji characters → 256 UTF-8 bytes,
        // but only 64 chars — should pass.
        profile.display_name = "👍".repeat(64);
        assert!(profile.validate().is_ok());

        // 65 emoji chars → should fail.
        profile.display_name = "👍".repeat(65);
        assert!(profile.validate().is_err());
    }

    #[test]
    fn shared_file_id_is_stable() {
        let now = SystemTime::now();
        let f1 = SharedFile::new("test.txt", 1024, "text/plain", now);
        let f2 = SharedFile::new("test.txt", 1024, "text/plain", now);
        assert_eq!(f1.id, f2.id, "identical metadata should produce same id");
    }

    #[test]
    fn shared_file_id_differs_for_different_metadata() {
        let now = SystemTime::now();
        let later = now + Duration::from_secs(60);
        let f1 = SharedFile::new("a.txt", 1024, "text/plain", now);
        let f2 = SharedFile::new("b.txt", 1024, "text/plain", later);
        assert_ne!(
            f1.id, f2.id,
            "different metadata should produce different ids"
        );
    }

    #[test]
    fn store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let key = test_key();

        // Create and save
        let mut store = UserProfileStore::empty_at(dir.path(), key);
        store.profile.display_name = "Alice".into();
        store.profile.bio = "Hello, world!".into();
        store.profile.file_sharing_enabled = true;
        store.profile.max_file_size = 50 * 1024 * 1024;
        store.profile.allowed_extensions = vec!["jpg".into(), "png".into()];
        store.add_shared_file(SharedFile::new(
            "photo.jpg",
            42_000,
            "image/jpeg",
            SystemTime::now(),
        ));
        store.save().unwrap();

        // Load and verify
        let loaded = UserProfileStore::load(dir.path(), key).unwrap();
        assert_eq!(loaded.profile.display_name, "Alice");
        assert_eq!(loaded.profile.bio, "Hello, world!");
        assert!(loaded.profile.file_sharing_enabled);
        assert_eq!(loaded.profile.max_file_size, 50 * 1024 * 1024);
        assert_eq!(loaded.profile.allowed_extensions, vec!["jpg", "png"]);
        assert_eq!(loaded.shared_files.len(), 1);
        assert_eq!(loaded.shared_files[0].filename, "photo.jpg");
        assert_eq!(loaded.shared_files[0].size, 42_000);
    }

    #[test]
    fn load_missing_file_creates_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let key = test_key();
        let store = UserProfileStore::load(dir.path(), key).unwrap();
        assert_eq!(store.profile.user_id, key);
        assert!(store.profile.display_name.is_empty());
        assert!(store.profile.bio.is_empty());
        assert!(!store.profile.file_sharing_enabled);
        assert!(!store.profile.allow_downloads);
        assert_eq!(store.profile.max_file_size, DEFAULT_MAX_FILE_SIZE);
    }

    #[test]
    fn save_validates_before_persisting() {
        let dir = tempfile::tempdir().unwrap();
        let key = test_key();
        let mut store = UserProfileStore::empty_at(dir.path(), key);
        store.profile.display_name = "x".repeat(65);
        let err = store.save().unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("display_name"),
            "save should fail validation: {msg}"
        );
        // File should NOT exist after failed save
        assert!(!store.file_path().exists());
    }

    #[test]
    fn corrupt_shared_file_entries_are_skipped() {
        // Write a profile.json with one valid and one invalid shared file
        let dir = tempfile::tempdir().unwrap();
        let key = test_key();
        let now = SystemTime::now();

        let mut store = UserProfileStore::empty_at(dir.path(), key);
        store.profile.display_name = "Test".into();
        store.add_shared_file(SharedFile::new("good.txt", 100, "text/plain", now));
        store.save().unwrap();

        // Manually corrupt the second entry by appending invalid JSON
        let path = store.file_path();
        let mut raw = fs::read_to_string(&path).unwrap();
        // Insert a corrupt entry before the closing bracket
        if let Some(pos) = raw.rfind(']') {
            let corrupted = r#", {"id": "bad", "filename": 42, "size": "not-a-number", "mime_type": "text/plain", "modified_time": 1000}"#;
            raw.insert_str(pos, corrupted);
        }
        fs::write(&path, &raw).unwrap();

        // Load should succeed, skipping the corrupt entry
        let loaded = UserProfileStore::load_or_default(dir.path(), key);
        assert_eq!(
            loaded.shared_files.len(),
            1,
            "corrupt entry should be skipped"
        );
        assert_eq!(loaded.shared_files[0].filename, "good.txt");
    }

    #[test]
    fn default_shared_folder_uses_documents() {
        let folder = default_shared_folder_path();
        assert!(folder.ends_with("Documents/Boru/Shared"));
    }

    #[test]
    fn remove_shared_file_works() {
        let now = SystemTime::now();
        let mut store = UserProfileStore::empty_at("/tmp", test_key());
        let f1 = SharedFile::new("a.txt", 10, "text/plain", now);
        let f2 = SharedFile::new("b.txt", 20, "text/plain", now);
        let id = f1.id.clone();
        store.add_shared_file(f1);
        store.add_shared_file(f2);
        assert_eq!(store.shared_files.len(), 2);

        assert!(store.remove_shared_file(&id));
        assert_eq!(store.shared_files.len(), 1);
        assert_eq!(store.shared_files[0].filename, "b.txt");

        assert!(!store.remove_shared_file("nonexistent"));
    }

    #[test]
    fn load_or_default_fallback_on_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let key = test_key();
        let path = dir.path().join(PROFILE_FILE_NAME);
        fs::write(&path, "this is not json").unwrap();

        let store = UserProfileStore::load_or_default(dir.path(), key);
        assert_eq!(store.profile.user_id, key);
        assert!(store.profile.display_name.is_empty());
        assert_eq!(store.shared_files.len(), 0);
    }

    #[test]
    fn placeholder_user_id_replaced_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let key = test_key();

        let placeholder =
            PublicKey::from_bytes(&[0u8; 32]).expect("32 zero bytes is a valid ed25519 public key");
        let store = UserProfileStore::empty_at(dir.path(), placeholder);
        store.save().unwrap();

        // Load with the real key — the placeholder should be replaced
        let loaded = UserProfileStore::load(dir.path(), key).unwrap();
        assert_eq!(
            loaded.profile.user_id, key,
            "placeholder should be replaced with the passed local_public"
        );
    }

    #[test]
    fn profile_mut_allows_mutation() {
        let key = test_key();
        let mut store = UserProfileStore::empty_at("/tmp", key);
        store.profile_mut().display_name = "Bob".into();
        assert_eq!(store.profile().display_name, "Bob");
    }

    #[test]
    fn set_profile_replaces_profile() {
        let key = test_key();
        let mut store = UserProfileStore::empty_at("/tmp", key);
        let mut new_profile = UserProfile::new(key);
        new_profile.display_name = "Charlie".into();
        store.set_profile(new_profile);
        assert_eq!(store.profile().display_name, "Charlie");
    }

    #[test]
    fn default_shared_file_fields() {
        let now = SystemTime::now();
        let file = SharedFile::new("doc.pdf", 5000, "application/pdf", now);
        assert!(!file.id.is_empty());
        assert_eq!(file.filename, "doc.pdf");
        assert_eq!(file.size, 5000);
        assert_eq!(file.mime_type, "application/pdf");
        assert!(file.hash.is_none());
        assert!(file.blob_id.is_none());
    }

    #[test]
    fn is_file_announce_allowed_returns_ok_when_enabled_and_in_limits() {
        let mut profile = UserProfile::new(test_key());
        profile.file_sharing_enabled = true;
        profile.max_file_size = 1024;
        profile.allowed_extensions = vec!["txt".into(), "jpg".into()];

        assert!(profile.is_file_announce_allowed(512, "txt").is_ok());
        assert!(profile.is_file_announce_allowed(512, ".txt").is_ok());
        assert!(profile.is_file_announce_allowed(512, "jpg").is_ok());
    }

    #[test]
    fn is_file_announce_allowed_rejects_when_disabled() {
        let mut profile = UserProfile::new(test_key());
        profile.file_sharing_enabled = false; // default is false
        let err = profile.is_file_announce_allowed(100, "txt").unwrap_err();
        assert!(err.contains("disabled"), "error: {err}");
    }

    #[test]
    fn is_file_announce_allowed_rejects_over_max_size() {
        let mut profile = UserProfile::new(test_key());
        profile.file_sharing_enabled = true;
        profile.max_file_size = 100;
        let err = profile.is_file_announce_allowed(200, "txt").unwrap_err();
        assert!(err.contains("exceeds"), "error: {err}");
    }

    #[test]
    fn is_file_announce_allowed_rejects_blocked_extension() {
        let mut profile = UserProfile::new(test_key());
        profile.file_sharing_enabled = true;
        profile.allowed_extensions = vec!["pdf".into()];
        let err = profile.is_file_announce_allowed(100, "jpg").unwrap_err();
        assert!(err.contains("not in the allowed list"), "error: {err}");
    }

    #[test]
    fn is_file_announce_allowed_empty_extensions_allows_all() {
        let mut profile = UserProfile::new(test_key());
        profile.file_sharing_enabled = true;
        profile.allowed_extensions = vec![]; // empty = all allowed
        assert!(profile.is_file_announce_allowed(100, "exe").is_ok());
        assert!(profile.is_file_announce_allowed(100, "zip").is_ok());
    }

    #[test]
    fn validate_received_filename_rejects_path_separators() {
        assert!(UserProfile::validate_received_filename("safe.txt").is_ok());
        assert!(UserProfile::validate_received_filename("safe_file.name").is_ok());

        assert!(UserProfile::validate_received_filename("../outside.txt").is_err());
        assert!(UserProfile::validate_received_filename("sub/file.txt").is_err());
        assert!(UserProfile::validate_received_filename("sub\\file.txt").is_err());
        assert!(UserProfile::validate_received_filename(".").is_err());
        assert!(UserProfile::validate_received_filename("..").is_err());
        assert!(UserProfile::validate_received_filename("").is_err());
    }

    #[test]
    fn over_limit_and_extension_blocked_flags_on_shared_file() {
        let now = SystemTime::now();
        let mut over = SharedFile::new("big.txt", 999, "text/plain", now);
        over.over_limit = true;
        let mut blocked = SharedFile::new("photo.jpg", 100, "image/jpeg", now);
        blocked.extension_blocked = true;
        let normal = SharedFile::new("ok.pdf", 100, "application/pdf", now);

        assert!(!over.is_announceable());
        assert!(!blocked.is_announceable());
        assert!(normal.is_announceable());
    }

    #[test]
    fn is_path_contained_accepts_same_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"data").unwrap();
        assert!(UserProfile::is_path_contained(&file_path, dir.path()));
    }

    #[test]
    fn is_path_contained_rejects_outside_path() {
        let dir = tempfile::tempdir().unwrap();
        let outside = std::env::temp_dir().join("outside.txt");
        // The outside path doesn't exist, so canonicalize will fail
        assert!(!UserProfile::is_path_contained(&outside, dir.path()));
    }

    #[test]
    fn symlink_inside_shared_folder_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.txt");
        std::fs::write(&target, b"data").unwrap();
        let link = dir.path().join("link.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let safe = UserProfile::symlink_is_safe(&link, dir.path());
        assert!(safe);
    }

    #[test]
    fn symlink_outside_shared_folder_is_unsafe() {
        let dir = tempfile::tempdir().unwrap();
        let outside = std::env::temp_dir().join("outside_ref.txt");
        std::fs::write(&outside, b"data").unwrap_or(());
        let link = dir.path().join("escape.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let safe = UserProfile::symlink_is_safe(&link, dir.path());
        // Only meaningful on unix where symlinks work
        #[cfg(unix)]
        assert!(!safe);
    }

    #[test]
    fn is_sharing_enabled_returns_file_sharing_enabled() {
        let mut profile = UserProfile::new(test_key());
        assert!(!profile.is_sharing_enabled());
        profile.file_sharing_enabled = true;
        assert!(profile.is_sharing_enabled());
    }

    #[test]
    fn is_download_allowed_returns_allow_downloads() {
        let mut profile = UserProfile::new(test_key());
        assert!(!profile.is_download_allowed());
        profile.allow_downloads = true;
        assert!(profile.is_download_allowed());
    }

    #[test]
    fn is_file_allowed_ok_when_sharing_enabled_and_in_limits() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("photo.jpg");
        std::fs::write(&file_path, b"small jpeg data").unwrap();

        let mut profile = UserProfile::new(test_key());
        profile.file_sharing_enabled = true;
        profile.max_file_size = 1024 * 1024; // 1 MB
        profile.allowed_extensions = vec!["jpg".into(), "png".into()];

        assert!(profile.is_file_allowed(&file_path).is_ok());
    }

    #[test]
    fn is_file_allowed_rejects_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("photo.jpg");
        std::fs::write(&file_path, b"data").unwrap();

        let profile = UserProfile::new(test_key()); // file_sharing_enabled = false
        let err = profile.is_file_allowed(&file_path).unwrap_err();
        assert!(err.contains("disabled"), "error: {err}");
    }

    #[test]
    fn is_file_allowed_rejects_over_max_size() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("data.bin");
        std::fs::write(&file_path, vec![0u8; 1000]).unwrap();

        let mut profile = UserProfile::new(test_key());
        profile.file_sharing_enabled = true;
        profile.max_file_size = 500;
        let err = profile.is_file_allowed(&file_path).unwrap_err();
        assert!(err.contains("exceeds"), "error: {err}");
    }

    #[test]
    fn is_file_allowed_rejects_blocked_extension() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("script.exe");
        std::fs::write(&file_path, b"fake exe").unwrap();

        let mut profile = UserProfile::new(test_key());
        profile.file_sharing_enabled = true;
        profile.allowed_extensions = vec!["pdf".into(), "txt".into()];
        let err = profile.is_file_allowed(&file_path).unwrap_err();
        assert!(err.contains("not in the allowed list"), "error: {err}");
    }

    #[test]
    fn is_file_allowed_empty_extensions_allows_all() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("some.unknown");
        std::fs::write(&file_path, b"data").unwrap();

        let mut profile = UserProfile::new(test_key());
        profile.file_sharing_enabled = true;
        profile.allowed_extensions = vec![]; // empty = all allowed
        assert!(profile.is_file_allowed(&file_path).is_ok());
    }

    #[test]
    fn is_file_allowed_nonexistent_path_returns_err() {
        let profile = UserProfile::new(test_key());
        let err = profile
            .is_file_allowed(&PathBuf::from("/nonexistent/file.txt"))
            .unwrap_err();
        assert!(err.contains("Cannot read file metadata"), "error: {err}");
    }

    #[test]
    fn is_file_allowed_empty_extension_allowed_when_list_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("noext");
        std::fs::write(&file_path, b"data").unwrap();

        let mut profile = UserProfile::new(test_key());
        profile.file_sharing_enabled = true;
        profile.allowed_extensions = vec![]; // empty = all allowed
        assert!(profile.is_file_allowed(&file_path).is_ok());
    }

    #[test]
    fn is_file_allowed_empty_extension_blocked_when_list_nonempty() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("noext");
        std::fs::write(&file_path, b"data").unwrap();

        let mut profile = UserProfile::new(test_key());
        profile.file_sharing_enabled = true;
        profile.allowed_extensions = vec!["txt".into()];
        let err = profile.is_file_allowed(&file_path).unwrap_err();
        assert!(err.contains("not in the allowed list"), "error: {err}");
    }

    #[test]
    fn check_file_announce_allowed_free_function() {
        let mut profile = UserProfile::new(test_key());
        profile.file_sharing_enabled = true;
        profile.max_file_size = 1024;
        assert!(check_file_announce_allowed(&profile, 512, "txt").is_ok());
        assert!(check_file_announce_allowed(&profile, 2048, "txt").is_err());
    }
}
