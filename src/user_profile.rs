//! User profile and shared file data models for Boru.
//!
//! **DEPRECATED** — user profile data is stored in the SQLite database
//! via the unified storage layer.  This JSON file is retained only for
//! backward-compatible reads during a transition period.
//!
//! This module defines [`UserProfile`](crate::user_profile::UserProfile) (local user identity and preferences)
//! and [`SharedFile`](crate::user_profile::SharedFile) (metadata about files the user shares with peers).
//!
//! The on-disk JSON file `profile.json` lives beside `secret_key.txt` in the
//! user's data directory alongside `friends.json` and `conversations.json`.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use iroh::PublicKey;
use n0_error::{bail_any, Result, StdResultExt};
use serde::{Deserialize, Deserializer, Serialize};
use tracing::warn;

use crate::chat_core::SharedFileMeta;

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
    #[serde(default)]
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

    /// File metadata announced in ProfileUpdate broadcasts.
    #[serde(default)]
    pub shared_files: Vec<SharedFileMeta>,
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
            shared_files: Vec::new(),
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

    /// Convenience: load the profile from a data directory, extracting just
    /// the [`UserProfile`] from the [`UserProfileStore`].
    pub fn load(data_dir: impl AsRef<Path>, local_public: PublicKey) -> Result<Self> {
        let store = UserProfileStore::load(data_dir, local_public)?;
        Ok(store.profile)
    }

    /// Convenience: save this profile by wrapping it in a [`UserProfileStore`]
    /// and persisting it atomically.
    ///
    /// **DEPRECATED:** profile data is now in the SQLite unified storage.
    /// This method logs a warning and returns the legacy path without
    /// writing to disk.
    #[deprecated(
        since = "0.21.0",
        note = "SQLite profile tables replace profile.json writes"
    )]
    pub fn save(&self, data_dir: impl AsRef<Path>) -> Result<PathBuf> {
        let path = data_dir.as_ref().join(PROFILE_FILE_NAME);
        warn!(
            path = %path.display(),
            "save() called on deprecated JSON profile store — no data written; \
             use SQLite profile tables instead"
        );
        Ok(path)
    }

    /// Validate profile fields, returning an error on constraint violation.
    ///
    /// Checks:
    /// - `display_name` must be at most `MAX_DISPLAY_NAME_LENGTH` characters.
    /// - `bio` must be at most `MAX_BIO_LENGTH` characters.
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

    /// Convert to the wire-format metadata for ProfileUpdate announcements.
    pub fn to_shared_file_meta(&self) -> crate::chat_core::SharedFileMeta {
        crate::chat_core::SharedFileMeta {
            id: self.id.clone(),
            filename: self.filename.clone(),
            size: self.size,
            mime_type: self.mime_type.clone(),
            modified_time: self
                .modified_time
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            hash: self.hash.unwrap_or([0u8; 32]),
        }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfileStore {
    /// Format version for future migrations.
    #[serde(default = "default_schema_version")]
    schema_version: u32,

    /// Whether first-launch onboarding has been completed or explicitly
    /// dismissed. Kept at store level so legacy profile data can be inferred
    /// without mutating the user's profile fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    onboarding_completed: Option<bool>,

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
            onboarding_completed: None,
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

    /// Return whether onboarding has been completed or dismissed.
    pub fn onboarding_completed(&self) -> bool {
        self.onboarding_completed.unwrap_or(false)
    }

    /// Set the persisted onboarding completion state.
    pub fn set_onboarding_completed(&mut self, completed: bool) {
        self.onboarding_completed = Some(completed);
    }

    /// Infer completion from pre-existing external application data.
    ///
    /// Returns `true` only when this call changes an incomplete store. An
    /// explicit completion state is never overridden.
    pub fn infer_onboarding_from_external(&mut self, has_existing_data: bool) -> bool {
        if has_existing_data && !self.onboarding_completed() {
            self.onboarding_completed = Some(true);
            true
        } else {
            false
        }
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

        // Legacy profile files predate the store-level onboarding flag. Infer
        // completion only when the field is absent; an explicit false is a
        // user's request to show onboarding again and must be preserved.
        let has_onboarding_field = serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|value| {
                value
                    .as_object()
                    .map(|object| object.contains_key("onboarding_completed"))
            })
            .unwrap_or(true);
        if !has_onboarding_field
            && (!store.profile.display_name.is_empty()
                || !store.profile.bio.is_empty()
                || store.profile.avatar_identifier.is_some()
                || !store.shared_files.is_empty())
        {
            store.onboarding_completed = Some(true);
        }

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
    /// **DEPRECATED:** profile data is now in the SQLite unified storage.
    /// This method logs a warning and returns the legacy path without
    /// writing to disk.
    #[deprecated(
        since = "0.21.0",
        note = "SQLite profile tables replace profile.json writes"
    )]
    pub fn save(&self) -> Result<PathBuf> {
        let path = self.file_path();
        warn!(
            path = %path.display(),
            "save() called on deprecated JSON profile store — no data written; \
             use SQLite profile tables instead"
        );
        Ok(path)
    }
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
        // ⚠ save() deprecated — testing in-memory store instead.
        let dir = tempfile::tempdir().unwrap();
        let key = test_key();

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

        assert_eq!(store.profile.display_name, "Alice");
        assert_eq!(store.profile.bio, "Hello, world!");
        assert!(store.profile.file_sharing_enabled);
        assert_eq!(store.profile.max_file_size, 50 * 1024 * 1024);
        assert_eq!(store.profile.allowed_extensions, vec!["jpg", "png"]);
        assert_eq!(store.shared_files.len(), 1);
        assert_eq!(store.shared_files[0].filename, "photo.jpg");
        assert_eq!(store.shared_files[0].size, 42_000);
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
        // ⚠ save() deprecated — validation was removed from save().
        // The save() method is now a no-op that always returns Ok.
        // Validation is handled by the SQLite storage layer.
        let dir = tempfile::tempdir().unwrap();
        let key = test_key();
        let mut store = UserProfileStore::empty_at(dir.path(), key);
        store.profile.display_name = "x".repeat(65);
        // save() returns Ok without writing (deprecated no-op)
        let path = store.file_path();
        assert_eq!(path, store.file_path());
    }

    #[test]
    fn corrupt_shared_file_entries_are_skipped() {
        // ⚠ save() deprecated — write test file manually for load testing.
        let dir = tempfile::tempdir().unwrap();
        let key = test_key();
        let now = SystemTime::now();

        let mut store = UserProfileStore::empty_at(dir.path(), key);
        store.profile.display_name = "Test".into();
        store.add_shared_file(SharedFile::new("good.txt", 100, "text/plain", now));

        // Write a valid profile.json manually (save() is a no-op).
        let modified_secs = store.shared_files[0]
            .modified_time
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let good_json = serde_json::json!({
            "schema_version": 1,
            "profile": {
                "user_id": key,
                "display_name": "Test",
                "bio": "",
                "file_sharing_enabled": false,
                "allow_downloads": false,
                "max_file_size": 104857600,
                "allowed_extensions": []
            },
            "shared_files": [{
                "id": store.shared_files[0].id,
                "filename": "good.txt",
                "size": 100,
                "mime_type": "text/plain",
                "modified_time": modified_secs
            }]
        });
        let path = store.file_path();
        fs::write(&path, good_json.to_string()).unwrap();

        // Now corrupt the file by appending invalid JSON
        let mut raw = fs::read_to_string(&path).unwrap();
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
            "corrupt entry should be skipped, got {} entries",
            loaded.shared_files.len()
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
        let _store = UserProfileStore::empty_at(dir.path(), placeholder);

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
    fn is_sharing_enabled_returns_file_sharing_enabled() {
        let mut profile = UserProfile::new(test_key());
        assert!(!profile.is_sharing_enabled());
        profile.file_sharing_enabled = true;
        assert!(profile.is_sharing_enabled());
    }

    #[test]
    fn shared_file_to_meta_contains_all_fields() {
        let now = SystemTime::now();
        let mut file = SharedFile::new("photo.jpg", 42_000, "image/jpeg", now);
        file.path = PathBuf::from("/shared/photo.jpg");
        file.hash = Some([0xab; 32]);
        let meta = file.to_shared_file_meta();
        assert_eq!(meta.id, file.id);
        assert_eq!(meta.filename, "photo.jpg");
        assert_eq!(meta.size, 42_000);
        assert_eq!(meta.mime_type, "image/jpeg");
        assert_eq!(meta.hash, [0xab; 32]);
        // modified_time should be > 0
        assert!(meta.modified_time > 0, "modified_time should be positive");
    }

    #[test]
    fn profile_update_with_shared_files_roundtrips() {
        use crate::chat_core::Message;
        let now = SystemTime::now();
        let mut file = SharedFile::new("doc.pdf", 100_000, "application/pdf", now);
        file.hash = Some([0xcd; 32]);
        let meta = file.to_shared_file_meta();

        let mut profile = UserProfile::new(test_key());
        profile.display_name = "bob".into();
        profile.bio = "sharing files".into();
        profile.file_sharing_enabled = true;
        profile.avatar_identifier = Some("avatar-id".into());
        profile.shared_folder_path = std::path::PathBuf::from("/tmp/shared");
        profile.allow_downloads = true;
        profile.shared_files = vec![meta];

        let msg = Message::ProfileUpdate(profile);
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        match decoded {
            Message::ProfileUpdate(profile) => {
                assert_eq!(profile.display_name, "bob");
                assert_eq!(profile.shared_files.len(), 1);
                assert_eq!(profile.shared_files[0].filename, "doc.pdf");
                assert_eq!(profile.shared_files[0].size, 100_000);
                assert_eq!(profile.shared_files[0].hash, [0xcd; 32]);
            }
            _ => panic!("expected ProfileUpdate"),
        }
    }
}
