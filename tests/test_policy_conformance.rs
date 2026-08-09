//! Policy conformance matrix (BORU-AUDIT-20).
//!
//! Runs the SAME hostile path/filename inputs through every public file
//! intake boundary and asserts the safety property holds at each one:
//!
//! - Traversal (`../`), absolute-path, symlink-escape and invalid-name
//!   inputs are consistently rejected (or sanitised to a name that stays
//!   inside the trusted root) at every boundary.
//! - A boundary that *accepts* a name must never produce a destination
//!   outside the caller's trusted directory.
//!
//! If you change policy behaviour in `src/file_policy.rs`,
//! `src/path_containment.rs`, `src/safe_destination.rs`,
//! `src/collection_transfer.rs`, `src/video_playback.rs`, or
//! `src/catalogue_model.rs`, update this matrix to keep the guarantee
//! explicit.

use std::fs;
use std::path::{Path, PathBuf};

use boru_core::catalogue_model::RemoteSharedFile;
use boru_core::collection_transfer::validate_path_component;
use boru_core::file_policy::admission;
use boru_core::path_containment::{is_path_contained, symlink_is_safe};
use boru_core::safe_destination::{
    prepare_download_destination, resolve_destination_with_policy, safe_destination_path,
    DestinationDecision, OverwritePolicy,
};
use boru_core::video_playback::validate_attachment_filename;
use tempfile::TempDir;

/// The canonical hostile input set.  Every boundary below must handle these
/// safely (reject or sanitise into a contained name).
const HOSTILE_NAMES: &[&str] = &[
    "../escape.txt",
    "../../../etc/passwd",
    "/etc/passwd",
    "sub/file.txt",
    "sub\\file.txt",
    "..",
    ".",
    "a/../b.txt",
    "C:autoexec.bat",
    "CON",
    "NUL",
    "",
    "   ",
    "safe.txt", // control: a plain name must always pass
    "report.pdf",
];

/// Assert that a destination-producing boundary never yields a path outside
/// `download_dir`, and rejects names that cannot be contained.
fn assert_destination_safe(download_dir: &Path, name: &str) {
    match safe_destination_path(download_dir, name, "content-hash") {
        Ok(dest) => {
            assert!(
                dest.starts_with(download_dir),
                "safe_destination_path accepted {name:?} but produced {dest:?} outside {download_dir:?}"
            );
            // The produced path must be a single component under the dir.
            assert_eq!(
                dest.parent(),
                Some(download_dir),
                "safe_destination_path produced nested path for {name:?}: {dest:?}"
            );
        }
        Err(_) => {
            // Rejection is also correct — the boundary must not produce an
            // escaping path.
        }
    }
}

/// Destination-producing boundaries must never escape the download dir.
#[test]
fn safe_destination_path_never_escapes() {
    let dir = TempDir::new().unwrap();
    for &name in HOSTILE_NAMES {
        assert_destination_safe(dir.path(), name);
    }
}

#[test]
fn prepare_download_destination_never_escapes() {
    let dir = TempDir::new().unwrap();
    for &name in HOSTILE_NAMES {
        match prepare_download_destination(dir.path(), name, "content-hash") {
            Ok(dest) => {
                assert!(
                    dest.starts_with(dir.path()),
                    "prepare_download_destination accepted {name:?} but produced {dest:?} outside"
                );
            }
            Err(_) => {}
        }
    }
}

#[test]
fn resolve_destination_with_policy_never_escapes() {
    let dir = TempDir::new().unwrap();
    for &name in HOSTILE_NAMES {
        for policy in [
            OverwritePolicy::KeepBoth,
            OverwritePolicy::Overwrite,
            OverwritePolicy::Skip,
        ] {
            match resolve_destination_with_policy(dir.path(), name, "content-hash", policy) {
                Ok(DestinationDecision::Use(dest)) => {
                    assert!(
                        dest.starts_with(dir.path()),
                        "resolve_destination_with_policy accepted {name:?} (policy {policy:?}) but produced {dest:?} outside"
                    );
                }
                Ok(DestinationDecision::Skip) => {}
                Err(_) => {}
            }
        }
    }
}

/// Path-component boundaries must reject traversal / separator names and
/// accept plain names.
#[test]
fn validate_path_component_rejects_hostile_names() {
    for &name in HOSTILE_NAMES {
        let is_plain = !name.is_empty()
            && !name.contains('/')
            && !name.contains('\\')
            && name != "."
            && name != "..";
        if is_plain {
            assert!(
                validate_path_component(name).is_ok(),
                "validate_path_component rejected plain name {name:?}"
            );
        } else {
            assert!(
                validate_path_component(name).is_err(),
                "validate_path_component accepted hostile name {name:?}"
            );
        }
    }
}

/// Attachment-name boundaries must never accept a name that could act as a
/// path component separator, absolute reference, or dot reference on the
/// CURRENT platform.  (What is "a separator" is platform-dependent: on
/// Unix `sub\file.txt` is a single safe component, on Windows it is two.)
#[test]
fn validate_attachment_filename_never_accepts_dangerous_names() {
    for &name in HOSTILE_NAMES {
        match validate_attachment_filename(name) {
            Ok(()) => {
                // Accepted ⇒ the name is a single, non-absolute, non-dot
                // component on this platform.
                let path = Path::new(name);
                assert!(
                    !name.is_empty()
                        && !path.is_absolute()
                        && path.components().count() == 1
                        && path.file_name().and_then(|n| n.to_str()) == Some(name)
                        && !matches!(name, "." | "..")
                        && !name.contains('\0')
                        && !name.chars().any(char::is_control),
                    "validate_attachment_filename accepted unsafe name {name:?}"
                );
            }
            Err(_) => {}
        }
    }
}

/// Path containment must fail closed for non-existent/escaping paths and
/// accept contained paths.
#[test]
fn path_containment_never_accepts_outside_paths() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("inside.txt"), b"data").unwrap();
    let outside = std::env::temp_dir().join(format!(
        "boru-conformance-outside-{}.txt",
        std::process::id()
    ));
    fs::write(&outside, b"data").unwrap();

    assert!(is_path_contained(
        &dir.path().join("inside.txt"),
        dir.path()
    ));
    assert!(!is_path_contained(&outside, dir.path()));

    // A symlink inside the root that points outside must be rejected.
    #[cfg(unix)]
    {
        let link = dir.path().join("escape-link.txt");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(
            !symlink_is_safe(&link, dir.path()),
            "symlink escaping root must be rejected"
        );
        assert!(
            !is_path_contained(&link, dir.path()),
            "containment must reject a symlink that resolves outside root"
        );

        // Symlink pointing inside the root is fine.
        let inside_link = dir.path().join("inside-link.txt");
        std::os::unix::fs::symlink(dir.path().join("inside.txt"), &inside_link).unwrap();
        assert!(symlink_is_safe(&inside_link, dir.path()));
        assert!(is_path_contained(&inside_link, dir.path()));
    }

    let _ = fs::remove_file(&outside);
}

/// Catalogue `display_name` validation must reject control / separator /
/// dot-reference names.
#[test]
fn catalogue_display_name_rejects_hostile_names() {
    for &name in HOSTILE_NAMES {
        let entry = RemoteSharedFile::new("hash-1", name, None, 100, "text/plain", None, 1);
        // A name with `/` or `\` or control chars or `.`/`..` is rejected.
        let hostile = name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
            || name.chars().any(char::is_control);
        if hostile {
            assert!(
                entry.validate().is_err(),
                "catalogue validate accepted hostile display_name {name:?}"
            );
        }
    }
}

/// The size + extension admission rule is the single canonical gate.
#[test]
fn file_policy_admission_is_the_canonical_rule() {
    let allowed: Vec<String> = vec!["txt".into(), "jpg".into()];

    // Over size limit.
    let a = admission(200, "txt", 100, &allowed);
    assert!(a.over_limit && !a.extension_blocked);
    assert!(!a.is_allowed());

    // Blocked extension.
    let a = admission(10, "exe", 100, &allowed);
    assert!(!a.over_limit && a.extension_blocked);
    assert!(!a.is_allowed());

    // In limits.
    let a = admission(10, "txt", 100, &allowed);
    assert!(a.is_allowed());

    // Empty allowlist = all allowed.
    let a = admission(10, "exe", 100, &[]);
    assert!(a.is_allowed());

    // Size boundary: equal is allowed, strictly greater is not.
    assert!(admission(100, "txt", 100, &allowed).is_allowed());
    assert!(admission(101, "txt", 100, &allowed).over_limit);
}

/// A hostile display name must never survive into a destination path — the
/// download boundary and the catalogue boundary must agree.
#[test]
fn hostile_names_never_become_escaped_destinations_end_to_end() {
    let dir = TempDir::new().unwrap();
    for &name in HOSTILE_NAMES {
        let dest = safe_destination_path(dir.path(), name, "content-hash");
        if let Ok(dest) = dest {
            assert!(
                dest.starts_with(dir.path()),
                "hostile name {name:?} escaped download dir: {dest:?}"
            );
            let file_name = dest
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            assert!(
                !file_name.contains('/') && !file_name.contains('\\'),
                "hostile name {name:?} produced separators in {file_name:?}"
            );
        }
    }
}

/// Regression: the legacy profile method used a case-SENSITIVE extension
/// comparison while the indexer used case-insensitive; the canonical rule is
/// case-insensitive.  A file named `report.JPG` with allowlist ["jpg"] must
/// be admitted.
#[test]
fn file_policy_is_case_insensitive_like_the_indexer_was() {
    let allowed: Vec<String> = vec!["jpg".into()];
    assert!(admission(10, "JPG", 100, &allowed).is_allowed());
    assert!(admission(10, "jpg", 100, &allowed).is_allowed());
    assert!(admission(10, ".JPG", 100, &allowed).is_allowed());
}
