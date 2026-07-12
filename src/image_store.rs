//! Secure, local storage for user-uploaded images.
//!
//! [`ImageStore`] keeps images below `<data_dir>/files`, with one directory
//! per user. User names and uploaded filenames are never used as path
//! components: user directories are keyed by a hash and image identifiers are
//! content hashes with a small allow-list of extensions.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use blake3::Hasher;
use n0_error::{Result, StdResultExt};

const FILES_DIR_NAME: &str = "files";
const CONTENT_HASH_HEX_LEN: usize = 64;

fn invalid_input(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message)
}

/// Local image store rooted at `<data_dir>/files`.
#[derive(Clone, Debug)]
pub struct ImageStore {
    root: PathBuf,
}

impl ImageStore {
    /// Create a store using the app data directory.
    ///
    /// The default app data directory is documented in the README; callers
    /// should pass the directory selected by `BORU_CHAT_DATA_DIR` (or
    /// the platform fallback) rather than relying on the current directory.
    pub fn at(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            root: data_dir.into().join(FILES_DIR_NAME),
        }
    }

    /// Create a store at an explicit files directory. This is useful for tests
    /// and for applications that already have a dedicated files root.
    pub fn from_files_dir(files_dir: impl Into<PathBuf>) -> Self {
        Self {
            root: files_dir.into(),
        }
    }

    /// Save an image and return a stable, portable identifier.
    ///
    /// The identifier has the form `<user-hash>/<content-hash>.<extension>`.
    /// Only `png`, `jpg`, `jpeg`, `gif`, `webp`, and `bmp` extensions are
    /// retained; all other or absent extensions use `.bin`.
    pub fn save_image(&self, user: &str, filename: &str, bytes: &[u8]) -> Result<String> {
        validate_user(user)?;
        if bytes.is_empty() {
            return Err(invalid_input("image must not be empty").into());
        }

        let user_hash = hash_hex(user.as_bytes());
        let content_hash = hash_hex(bytes);
        let extension = safe_extension(filename);
        let relative = format!("{user_hash}/{content_hash}.{extension}");
        let user_dir = self.root.join(&user_hash);
        fs::create_dir_all(&user_dir).std_context("create image user directory")?;
        if user_dir.is_symlink() {
            return Err(invalid_input("image user directory is a symlink").into());
        }
        set_private_dir(&self.root);
        set_private_dir(&user_dir);

        let destination = user_dir.join(format!("{content_hash}.{extension}"));
        if destination.exists() {
            if destination.is_symlink() {
                return Err(invalid_input("image destination path is a symlink").into());
            }
            if destination.is_file() {
                return Ok(relative);
            }
            return Err(invalid_input("image destination path is not a regular file").into());
        }
        let temp = user_dir.join(format!(".{content_hash}.{extension}.tmp"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .std_context("create temporary image file")?;
        let write_result = (|| {
            file.write_all(bytes).std_context("write image")?;
            file.sync_all().std_context("sync image")?;
            Ok(())
        })();
        drop(file);
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        fs::rename(&temp, &destination).std_context("commit image file")?;
        set_private_file(&destination);
        Ok(relative)
    }

    /// Validate and return a previously returned storage-relative identifier.
    ///
    /// The returned path is relative to this store's configured files root;
    /// callers never receive an absolute filesystem path from this API.
    ///
    /// To read the actual file, use [`resolve_absolute_path`](Self::resolve_absolute_path)
    /// which combines this relative path with the configured files root.
    pub fn resolve_image(&self, user: &str, identifier: &str) -> Result<PathBuf> {
        validate_user(user)?;
        let user_hash = hash_hex(user.as_bytes());
        let (id_user, filename) = identifier
            .split_once('/')
            .ok_or_else(|| invalid_input("invalid image identifier"))?;
        if id_user != user_hash || !valid_image_filename(filename) {
            return Err(invalid_input("invalid image identifier").into());
        }
        let relative = PathBuf::from(id_user).join(filename);
        let path = self.root.join(&relative);
        if self.root.join(id_user).is_symlink() || path.is_symlink() {
            return Err(invalid_input("image path is a symlink").into());
        }
        Ok(relative)
    }

    /// Resolve an image identifier to its absolute filesystem path.
    ///
    /// This is the same validation as [`resolve_image`](Self::resolve_image)
    /// but returns the full absolute path instead of a relative one, so
    /// callers can read the file directly without duplicating the files-root
    /// logic.  Returns a clear error for missing users, invalid identifiers,
    /// or symlink-based traversal attempts.
    pub fn resolve_absolute_path(&self, user: &str, identifier: &str) -> Result<PathBuf> {
        let relative = self.resolve_image(user, identifier)?;
        let absolute = self.root.join(&relative);
        if absolute.is_symlink() {
            return Err(invalid_input("resolved image path is a symlink").into());
        }
        Ok(absolute)
    }

    /// Return whether an image identifier resolves to a regular file.
    pub fn image_exists(&self, user: &str, identifier: &str) -> Result<bool> {
        let path = self.root.join(self.resolve_image(user, identifier)?);
        Ok(path.is_file())
    }

    /// Delete an image. Returns whether a file was removed.
    pub fn delete_image(&self, user: &str, identifier: &str) -> Result<bool> {
        let path = self.root.join(self.resolve_image(user, identifier)?);
        match fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error).std_context("delete image"),
        }
    }
}

fn validate_user(user: &str) -> Result<()> {
    if user.is_empty() {
        return Err(invalid_input("user identifier must not be empty").into());
    }
    Ok(())
}

fn hash_hex(bytes: &[u8]) -> String {
    let mut hasher = Hasher::new();
    hasher.update(bytes);
    hasher.finalize().to_hex().to_string()
}

fn safe_extension(filename: &str) -> &'static str {
    let extension = Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => "png",
        "jpg" => "jpg",
        "jpeg" => "jpeg",
        "gif" => "gif",
        "webp" => "webp",
        "bmp" => "bmp",
        _ => "bin",
    }
}

fn valid_image_filename(filename: &str) -> bool {
    let (stem, extension) = match filename.rsplit_once('.') {
        Some(value) => value,
        None => return false,
    };
    stem.len() == CONTENT_HASH_HEX_LEN
        && stem.bytes().all(|byte| byte.is_ascii_hexdigit())
        && matches!(
            extension,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "bin"
        )
}

fn set_private_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
}

fn set_private_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_persists_per_user_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        let id = store.save_image("alice", "photo.png", b"pixels").unwrap();
        assert!(id.starts_with(&format!("{}/", hash_hex(b"alice"))));
        assert!(store.image_exists("alice", &id).unwrap());
        let relative = store.resolve_image("alice", &id).unwrap();
        assert!(!relative.is_absolute());
        assert_eq!(fs::read(store.root.join(relative)).unwrap(), b"pixels");
    }

    #[test]
    fn users_are_isolated() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        let id = store.save_image("alice", "photo.png", b"pixels").unwrap();
        assert!(!store.image_exists("bob", &id).unwrap_or(false));
    }

    #[test]
    fn invalid_filename_does_not_survive_identifier() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        let id = store
            .save_image("alice", "../../secret.exe", b"pixels")
            .unwrap();
        assert!(id.ends_with(".bin"));
        assert!(store.resolve_image("alice", "../../secret.exe").is_err());
    }

    #[test]
    fn traversal_and_symlink_like_identifiers_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        for id in ["../secret", "x/../../secret.bin", "alice/../secret.bin", ""] {
            assert!(store.resolve_image("alice", id).is_err(), "accepted {id:?}");
        }
    }

    #[test]
    fn delete_removes_image() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        let id = store.save_image("alice", "photo.jpg", b"pixels").unwrap();
        assert!(store.delete_image("alice", &id).unwrap());
        assert!(!store.image_exists("alice", &id).unwrap());
        assert!(!store.delete_image("alice", &id).unwrap());
    }

    #[test]
    fn save_image_reuses_existing_cached_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        let id1 = store.save_image("alice", "photo.jpg", b"pixels").unwrap();
        let path = store.resolve_absolute_path("alice", &id1).unwrap();
        let initial_mtime = fs::metadata(&path).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_secs(1));

        let id2 = store.save_image("alice", "photo.jpg", b"pixels").unwrap();
        let final_mtime = fs::metadata(&path).unwrap().modified().unwrap();

        assert_eq!(id1, id2);
        assert_eq!(
            initial_mtime, final_mtime,
            "cached image should not be rewritten"
        );
    }

    #[test]
    fn resolve_absolute_path_returns_readable_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        let id = store
            .save_image("alice", "photo.png", b"hello-world")
            .unwrap();
        let abs = store.resolve_absolute_path("alice", &id).unwrap();
        assert!(abs.is_absolute());
        assert!(abs.starts_with(dir.path().join("files")));
        assert_eq!(fs::read(&abs).unwrap(), b"hello-world");
    }

    #[test]
    fn resolve_absolute_path_rejects_empty_user() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        let err = store.resolve_absolute_path("", "x/y.png").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("empty"), "error should mention empty: {msg}");
    }

    #[test]
    fn resolve_absolute_path_rejects_invalid_identifier() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        let err = store
            .resolve_absolute_path("alice", "../secret")
            .unwrap_err();
        assert!(
            format!("{err}").contains("invalid"),
            "expected invalid identifier error"
        );
    }

    #[test]
    fn resolve_absolute_path_rejects_traversal_identifier() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        let err = store
            .resolve_absolute_path("alice", "meow/../../secret.bin")
            .unwrap_err();
        assert!(
            format!("{err}").contains("invalid"),
            "expected invalid identifier error"
        );
    }

    #[test]
    fn from_files_dir_absolute_path_is_under_files_root() {
        let dir = tempfile::tempdir().unwrap();
        let custom = dir.path().join("custom-images");
        let store = ImageStore::from_files_dir(&custom);
        let id = store.save_image("bob", "pic.gif", b"xyz").unwrap();
        let abs = store.resolve_absolute_path("bob", &id).unwrap();
        assert!(
            abs.starts_with(&custom),
            "abs path should be under custom root"
        );
        assert_eq!(fs::read(&abs).unwrap(), b"xyz");
    }

    // ── Additional isolation & traversal protection tests ────────────

    #[test]
    fn empty_user_rejected_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        let err = store.save_image("", "photo.png", b"data").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("empty"), "error should mention empty: {msg}");
    }

    #[test]
    fn empty_bytes_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        let err = store.save_image("alice", "photo.png", b"").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("empty") || msg.contains("empty"),
            "error should mention empty: {msg}"
        );
    }

    #[test]
    fn lazy_user_dir_creation() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        let user_hash = hash_hex(b"alice");
        let user_dir = store.root.join(&user_hash);
        // Directory does not exist before first save.
        assert!(!user_dir.exists(), "user dir should not exist yet");
        let id = store.save_image("alice", "photo.png", b"data").unwrap();
        // Directory exists after save.
        assert!(user_dir.is_dir(), "user dir should exist after save");
        assert!(user_dir.join(id.split_once('/').unwrap().1).is_file());
        // Resolve against a user that never saved does not create a directory.
        let bob_hash = hash_hex(b"bob");
        assert!(!store.root.join(&bob_hash).exists());
    }

    #[test]
    fn users_cannot_resolve_each_others_images() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        let id = store.save_image("alice", "photo.png", b"pixels").unwrap();
        // Bob cannot resolve Alice's identifier.
        assert!(
            store.resolve_image("bob", &id).is_err(),
            "bob should not resolve alice's image"
        );
        // Bob cannot get the absolute path either.
        assert!(
            store.resolve_absolute_path("bob", &id).is_err(),
            "bob should not get absolute path of alice's image"
        );
        // Bob cannot even check existence.
        assert!(
            !store.image_exists("bob", &id).unwrap_or(false),
            "bob's exists check on alice's image should fail closed"
        );
    }

    #[test]
    fn unsafe_extensions_normalised_to_bin() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        // Known image extensions are preserved.
        for (ext, label) in [
            ("png", "png"),
            ("jpg", "jpg"),
            ("jpeg", "jpeg"),
            ("gif", "gif"),
            ("webp", "webp"),
            ("bmp", "bmp"),
        ] {
            let id = store
                .save_image("alice", &format!("img.{ext}"), b"x")
                .unwrap();
            assert!(
                id.ends_with(&format!(".{label}")),
                "expected .{label} for .{ext}, got {id}"
            );
        }
        // Unsafe / unrecognised extensions are normalised to .bin.
        for ext in [
            "exe", "sh", "py", "svg", "html", "js", "pdf", "doc", "com", "scr",
        ] {
            let id = store
                .save_image("alice", &format!("bad.{ext}"), b"x")
                .unwrap();
            assert!(id.ends_with(".bin"), "expected .bin for .{ext}, got {id}");
        }
        // No extension → .bin.
        let id = store.save_image("alice", "README", b"x").unwrap();
        assert!(
            id.ends_with(".bin"),
            "no-extension should become .bin, got {id}"
        );
        // Double extension: only the last component matters.
        let id = store.save_image("alice", "photo.png.exe", b"x").unwrap();
        assert!(id.ends_with(".bin"), ".exe wins → .bin, got {id}");
    }

    #[test]
    fn windows_backslash_identifiers_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        // These are invalid identifiers regardless of platform.
        for id in [
            "..\\secret.bin",
            "..\\..\\secret.bin",
            "alice\\..\\secret.bin",
            "hash\\..\\..\\secret.bin",
        ] {
            assert!(
                store.resolve_image("alice", id).is_err(),
                "accepted backslash traversal {id:?}"
            );
            assert!(
                store.resolve_absolute_path("alice", id).is_err(),
                "accepted abs backslash traversal {id:?}"
            );
        }
    }

    #[test]
    fn encoded_traversal_identifiers_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        // URL-encoded or otherwise encoded traversal strings do NOT get
        // decoded by ImageStore — they are literal path components that
        // fail the content-hash identifier format check.
        for id in [
            "%2e%2e%2fsecret.bin",
            "%2e%2e%5csecret.bin",
            "..%2fsecret.bin",
            "hash/%2e%2e%2fsecret.bin",
        ] {
            assert!(
                store.resolve_image("alice", id).is_err(),
                "accepted encoded traversal {id:?}"
            );
        }
    }

    #[test]
    fn absolute_path_identifiers_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        for id in [
            "/etc/passwd",
            "/etc/passwd.bin",
            "C:\\Windows\\system32.bin",
        ] {
            assert!(
                store.resolve_image("alice", id).is_err(),
                "accepted absolute identifier {id:?}"
            );
            assert!(
                store.resolve_absolute_path("alice", id).is_err(),
                "accepted absolute abs identifier {id:?}"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn actual_symlink_rejected() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        // Create the files root and a user directory as a symlink to an external
        // location.  The parent must exist (save_image creates it implicitly)
        // so we create it first to set up the symlink.
        let user_hash = hash_hex(b"alice");
        let user_dir = store.root.join(&user_hash);
        fs::create_dir_all(&store.root).unwrap();
        let external = dir.path().join("external");
        fs::create_dir(&external).unwrap();
        symlink(&external, &user_dir).unwrap();
        // save_image should detect and reject the symlink.
        let err = store.save_image("alice", "photo.png", b"data").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("symlink"),
            "error should mention symlink: {msg}"
        );
        // The file should not have been written to the external directory.
        assert_eq!(
            fs::read_dir(&external).unwrap().count(),
            0,
            "external dir should be empty after rejected save"
        );
    }

    #[test]
    fn all_files_under_storage_root() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::at(dir.path());
        let root = dir.path().join("files");
        let id1 = store.save_image("alice", "a.png", b"aaa").unwrap();
        let id2 = store.save_image("alice", "b.jpg", b"bbb").unwrap();
        let id3 = store.save_image("bob", "c.gif", b"ccc").unwrap();
        for (user, id) in [("alice", &id1), ("alice", &id2), ("bob", &id3)] {
            let abs = store.resolve_absolute_path(user, id).unwrap();
            assert!(
                abs.starts_with(&root),
                "{} should be under {:?}",
                abs.display(),
                root
            );
            assert!(abs.is_file(), "{} should exist", abs.display());
        }
        // Walk the entire temp dir and verify nothing lives outside `files/`.
        let entries = walk_dir(dir.path());
        for entry in entries {
            let rel = entry.strip_prefix(dir.path()).unwrap();
            let components: Vec<_> = rel.components().collect();
            assert!(
                components.first().map(|c| c.as_os_str()) == Some(std::ffi::OsStr::new("files")),
                "file {:?} is outside the files/ root",
                rel
            );
        }
    }

    fn walk_dir(path: &Path) -> Vec<PathBuf> {
        let mut result = Vec::new();
        if path.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    result.extend(walk_dir(&path));
                } else {
                    result.push(path);
                }
            }
        }
        result
    }
}
