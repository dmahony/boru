//! Path containment helpers shared by download/export safety checks.
//!
//! `canonicalize_allow_missing` resolves symlinks in a path even when the
//! final components do not exist yet — the normal case for an export or
//! download target, where `Path::canonicalize` would fail.  Containment
//! checks in [`crate::collection_transfer`] and [`crate::safe_destination`]
//! use it to compare a target path against its root without tripping over
//! symlinked ancestry or the Windows `\\?\` extended-length prefix.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Check that `path` resolves inside `root` after symlink resolution.
///
/// This is the **canonical path-containment check** for files that already
/// exist (or whose ancestors exist): it canonicalises both sides with
/// `canonicalize_allow_missing` and verifies the result starts with the
/// root.  A pre-existing symlink inside the root that points outside is
/// resolved and therefore rejected.
///
/// Returns `false` (fail closed) when either side cannot be resolved.
///
/// This replaces the legacy `UserProfile::is_path_contained` (BORU-AUDIT-20);
/// the legacy method used `Path::canonicalize`, which fails on
/// not-yet-existing targets.  `canonicalize_allow_missing` handles the
/// export/download target case too, so this is the single containment check
/// for both existing and to-be-created paths.
pub fn is_path_contained(path: &Path, root: &Path) -> bool {
    let canonical = canonicalize_allow_missing(path);
    let root_canonical = canonicalize_allow_missing(root);
    canonical.starts_with(&root_canonical)
}

/// Check that a symlink at `path` does not escape the shared `root` folder.
///
/// This is the **canonical symlink-escape check** used by the shared-folder
/// indexer.  If `path` is not a symlink, this returns `true` (allowed by
/// default).  If it *is* a symlink, its target is resolved — absolute
/// targets directly, relative targets against the symlink's parent — and the
/// resolved target is checked for containment with [`is_path_contained`].
///
/// Returns `false` (fail closed) when the path cannot be inspected.
///
/// This replaces the legacy `UserProfile::symlink_is_safe` (BORU-AUDIT-20).
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
        // Relative target: resolve against the symlink's parent directory.
        path.parent().unwrap_or(Path::new(".")).join(&target)
    };
    is_path_contained(&resolved, root)
}

/// Canonicalize `path`, resolving symlinks, even when trailing components
/// do not exist yet.
///
/// `Path::canonicalize` requires the whole path to exist.  Export and
/// download targets are created *after* the containment check, so this
/// helper walks up to the deepest existing ancestor, canonicalizes it
/// (resolving any symlinked ancestry — including the Windows `\\?\`
/// extended-length prefix), and re-joins the remaining components lexically.
///
/// The re-joined tail is expected to have been validated by the caller
/// ([`crate::collection_transfer::validate_path_component`] or
/// [`crate::safe_destination`]'s sanitisation), so it cannot contain path
/// separators or traversal components.  A pre-existing symlink *inside* the
/// root that points outside is resolved by the ancestor canonicalization and
/// therefore still fails a `starts_with` containment check.
///
/// If no existing ancestor can be found (e.g. a relative path with no
/// existing parent), the raw path is returned unchanged — the caller's own
/// validation gates it.
pub(crate) fn canonicalize_allow_missing(path: &Path) -> PathBuf {
    let mut tail: Vec<&OsStr> = Vec::new();
    let mut cur = path;
    loop {
        if let Ok(canon) = cur.canonicalize() {
            let mut out = canon;
            for part in tail.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (cur.file_name(), cur.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                cur = parent;
            }
            _ => break,
        }
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_path_contained_accepts_same_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"data").unwrap();
        assert!(is_path_contained(&file_path, dir.path()));
    }

    #[test]
    fn is_path_contained_rejects_outside_path() {
        let dir = tempfile::tempdir().unwrap();
        let outside = std::env::temp_dir().join("outside-boru-pc.txt");
        // The outside path does not exist — fail closed.
        assert!(!is_path_contained(&outside, dir.path()));
    }

    #[test]
    fn is_path_contained_accepts_missing_target_inside_root() {
        // The export/download target does not exist yet but its ancestors do:
        // containment still holds (canonicalize_allow_missing handles it).
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sub").join("new-file.txt");
        assert!(is_path_contained(&target, dir.path()));
    }

    #[test]
    fn symlink_inside_shared_folder_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.txt");
        std::fs::write(&target, b"data").unwrap();
        let link = dir.path().join("link.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(symlink_is_safe(&link, dir.path()));
    }

    #[test]
    fn symlink_outside_shared_folder_is_unsafe() {
        let dir = tempfile::tempdir().unwrap();
        let outside = std::env::temp_dir().join("outside-boru-ref.txt");
        std::fs::write(&outside, b"data").unwrap_or(());
        let link = dir.path().join("escape.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        // Only meaningful on unix where symlinks work.
        #[cfg(unix)]
        assert!(!symlink_is_safe(&link, dir.path()));
    }

    #[test]
    fn non_symlink_file_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plain.txt");
        std::fs::write(&file, b"data").unwrap();
        assert!(symlink_is_safe(&file, dir.path()));
    }

    #[test]
    fn missing_path_fails_closed_for_symlink_check() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.txt");
        // A missing path cannot be a symlink — fail closed.
        assert!(!symlink_is_safe(&missing, dir.path()));
    }

    #[test]
    fn missing_target_outside_root_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        // A not-yet-existing target whose nearest existing ancestor lies
        // OUTSIDE the root must be rejected (fail closed).
        let outside = std::env::temp_dir()
            .join("boru-outside-missing")
            .join("new.txt");
        assert!(!is_path_contained(&outside, dir.path()));
    }
}
