//! Path containment helpers shared by download/export safety checks.
//!
//! [`canonicalize_allow_missing`] resolves symlinks in a path even when the
//! final components do not exist yet — the normal case for an export or
//! download target, where `Path::canonicalize` would fail.  Containment
//! checks in [`crate::collection_transfer`] and [`crate::safe_destination`]
//! use it to compare a target path against its root without tripping over
//! symlinked ancestry or the Windows `\\?\` extended-length prefix.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

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
