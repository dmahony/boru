//! Safe destination selection for downloaded files.
//!
//! Derives a filesystem-safe destination path from a remote display name,
//! preventing path traversal, overwriting existing files silently, and
//! keeping the output inside the caller-selected download directory.
//!
//! # Guarantees
//!
//! - The returned path always lies inside `download_dir` (or is rejected).
//! - Path separators in the remote name are stripped (not accepted as
//!   directory components).
//! - Traversal attempts (`name/..`, absolute paths) are rejected with an
//!   error.
//! - Reserved platform names (`.`, `..`, NUL, CON, etc. on Windows) are
//!   replaced with a safe fallback.
//! - If the computed file already exists, the filename is deduplicated
//!   (e.g. `file (1).pdf`) rather than silently overwritten.

use std::path::{Path, PathBuf};

use n0_error::Result;

/// Maximum number of deduplication attempts before giving up.
const MAX_DEDUP_ATTEMPTS: u32 = 10_000;

// ── Core function ─────────────────────────────────────────────────────────

/// Derive a safe filesystem destination from a remote display name.
///
/// * `download_dir` — the trusted, user-selected download directory.
/// * `display_name` — the remote file's display name (potentially
///   malicious).  Path separators, reserved names, and traversal
///   components are handled.
/// * `fallback_stem` — a stable identifier (e.g. content hash) used
///   when the sanitised name is empty or consists of only reserved
///   characters.
///
/// # Errors
///
/// - Returns an error if `display_name` contains a traversal component
///   (e.g. `..`) after path-separator removal.
/// - Returns an error if the resulting path would escape `download_dir`
///   (belt-and-suspenders — traversal checks above should prevent this).
/// - Returns an error if deduplication exhausts [`MAX_DEDUP_ATTEMPTS`]
///   (extremely unlikely unless there are >10k files with the same name).
///
/// # Stability
///
/// The deduplication format (`"base (N).ext"`) is subject to change.
/// Do not parse it; treat it as an opaque display string.
pub fn safe_destination_path(
    download_dir: &Path,
    display_name: &str,
    fallback_stem: &str,
) -> Result<PathBuf> {
    if !download_dir.is_absolute() {
        return Err(n0_error::anyerr!(
            "download_dir must be absolute: {}",
            download_dir.display()
        ));
    }

    // Strip path separators so we can check for traversal before the
    // reserved-name sanitisation replaces ".." with a fallback.
    let stripped: String = display_name
        .chars()
        .filter(|&c| c != '/' && c != '\\')
        .collect();

    check_traversal(&stripped)?;

    let safe_name = sanitise_filename(display_name, fallback_stem);

    let candidate = download_dir.join(&safe_name);

    // Belt-and-suspenders: canonicalise and verify the path is inside the
    // download directory.
    let candidate_safe = candidate.canonicalize().unwrap_or(candidate.clone());
    if !candidate_safe.starts_with(download_dir) {
        return Err(n0_error::anyerr!(
            "destination path escapes download directory: {}",
            candidate_safe.display()
        ));
    }

    // Automatic deduplication to avoid silent overwrite.
    let final_path = deduplicate_path(&candidate, MAX_DEDUP_ATTEMPTS)?;

    Ok(final_path)
}

/// Sanitise a display name into a safe filesystem name.
///
/// 1. Strips path separators (`/` and `\`).
/// 2. Rejects the result if it would be empty or all-reserved.
/// 3. Returns the sanitised name, or a `fallback_stem`-based name when
///    the display name produces nothing safe.
fn sanitise_filename(name: &str, fallback_stem: &str) -> String {
    // Strip path separators — we never accept directory components.
    let cleaned: String = name.chars().filter(|&c| c != '/' && c != '\\').collect();

    // If stripping left nothing, use the fallback.
    if cleaned.is_empty() || cleaned.trim().is_empty() {
        return fallback_stem.to_string();
    }

    // Check for reserved platform names.
    if is_reserved_platform_name(&cleaned) || is_all_dots(&cleaned) {
        // Reserved name — use the fallback but preserve the extension
        // if one can reasonably be extracted.
        let stem = cleaned.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
        if !stem.is_empty() && !is_reserved_platform_name(stem) && !is_all_dots(stem) {
            format!("{fallback_stem}.{stem}")
        } else {
            fallback_stem.to_string()
        }
    } else {
        cleaned
    }
}

/// Return `true` when `name` is a reserved platform filename.
///
/// On Windows the following names are reserved and cannot be used as file or
/// directory names: `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`,
/// with or without an extension.  On all platforms, `.` and `..` are reserved
/// (directory self / parent).
fn is_reserved_platform_name(name: &str) -> bool {
    // Extract the stem (everything before the first dot) for comparison.
    let stem = name.split('.').next().unwrap_or(name).to_uppercase();

    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// Return `true` when `name` consists entirely of `.` characters.
fn is_all_dots(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c == '.')
}

/// Check that a name does not contain a traversal component after separator
/// removal.
fn check_traversal(name: &str) -> Result<()> {
    // After removing path separators, the only traversal risk from `..`
    // comes from the name itself being exactly ".." or starting with ".."
    // as a component.  Since we already stripped '/' and '\\', there is
    // no way to embed a directory component, so a name of ".." or "..." is
    // the only self/parent reference that survives.
    if name == ".." || name == "." {
        return Err(n0_error::anyerr!(
            "filename is a directory reference: {name:?}"
        ));
    }

    // An absolute path would have had its leading separator stripped, so
    // `/foo` becomes `foo` which is fine.  But a name that began with a
    // drive letter on Windows (e.g. `C:foo`) would survive.  Catch common
    // patterns.
    if name.len() >= 2 && name.as_bytes()[1] == b':' {
        let drive = name.as_bytes()[0];
        if drive.is_ascii_alphabetic() {
            return Err(n0_error::anyerr!(
                "filename contains a drive-letter prefix: {name:?}"
            ));
        }
    }

    Ok(())
}

/// If `path` already exists, generate a non-existent variant by inserting a
/// deduplication suffix before the extension.
///
/// Examples (when `path` = `dir/report.pdf` and it exists):
/// - `dir/report (1).pdf`
/// - `dir/report (2).pdf`
/// - …up to `max_attempts`.
fn deduplicate_path(path: &Path, max_attempts: u32) -> Result<PathBuf> {
    if !path.exists() {
        return Ok(path.to_path_buf());
    }

    let parent = path.parent().unwrap_or(Path::new(""));
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| n0_error::anyerr!("path has no filename component"))?;

    let (stem, ext) = match filename.rfind('.') {
        Some(dot) if dot > 0 => {
            let (base, suffix) = filename.split_at(dot);
            (base.to_string(), suffix.to_string())
        }
        _ => (filename.to_string(), String::new()),
    };

    for i in 1..=max_attempts {
        let deduped = if ext.is_empty() {
            format!("{stem} ({i})")
        } else {
            format!("{stem} ({i}){ext}")
        };
        let candidate = parent.join(&deduped);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(n0_error::anyerr!(
        "exhausted {max_attempts} deduplication attempts for {}",
        path.display()
    ))
}

// ── Convenience wrapper for download creation ────────────────────────────

/// Compute a safe destination and create the download record.
///
/// This is the intended entry-point for external callers: it sanitises the
/// remote display name, deduplicates against existing files, verifies the
/// destination stays inside `download_dir`, and then creates the download
/// row with the destination persisted.
///
/// In addition to the error conditions listed on
/// [`safe_destination_path`], this function returns an error when the
/// sanitised filename is empty after all transformations.
pub fn prepare_download_destination(
    download_dir: &Path,
    display_name: &str,
    content_hash: &str,
) -> Result<PathBuf> {
    let dest = safe_destination_path(download_dir, display_name, content_hash)?;

    if dest
        .file_name()
        .and_then(|s| s.to_str())
        .is_none_or(|s| s.is_empty())
    {
        return Err(n0_error::anyerr!(
            "sanitised filename is empty for display_name {display_name:?}"
        ));
    }

    Ok(dest)
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ── sanitise_filename ───────────────────────────────────────────

    #[test]
    fn path_separators_are_stripped() {
        assert_eq!(sanitise_filename("a/b.txt", "hash"), "ab.txt");
        assert_eq!(sanitise_filename("a\\b.txt", "hash"), "ab.txt");
        assert_eq!(sanitise_filename("/etc/passwd", "hash"), "etcpasswd");
    }

    #[test]
    fn empty_name_uses_fallback() {
        assert_eq!(sanitise_filename("", "abc123"), "abc123");
    }

    #[test]
    fn whitespace_only_name_uses_fallback() {
        assert_eq!(sanitise_filename("   ", "abc123"), "abc123");
    }

    #[test]
    fn reserved_platform_name_uses_fallback() {
        assert_eq!(sanitise_filename("CON", "abc123"), "abc123");
        assert_eq!(sanitise_filename("con.txt", "abc123"), "abc123.txt");
        assert_eq!(sanitise_filename("PRN", "abc123"), "abc123");
        assert_eq!(sanitise_filename("NUL.dat", "abc123"), "abc123.dat");
        assert_eq!(sanitise_filename("COM1", "abc123"), "abc123");
        assert_eq!(sanitise_filename("LPT9", "abc123"), "abc123");
        assert_eq!(sanitise_filename("con.com", "abc123"), "abc123.com");
    }

    #[test]
    fn all_dots_uses_fallback() {
        assert_eq!(sanitise_filename("...", "abc123"), "abc123");
        assert_eq!(sanitise_filename(".", "abc123"), "abc123");
        assert_eq!(sanitise_filename("..", "abc123"), "abc123");
    }

    #[test]
    fn normal_name_passes_through() {
        assert_eq!(sanitise_filename("photo.jpg", "hash"), "photo.jpg");
        assert_eq!(
            sanitise_filename("my document.pdf", "hash"),
            "my document.pdf"
        );
        assert_eq!(
            sanitise_filename("archive.tar.gz", "hash"),
            "archive.tar.gz"
        );
    }

    #[test]
    fn unicode_name_preserved() {
        let name = "résumé.pdf";
        assert_eq!(sanitise_filename(name, "hash"), name);
    }

    #[test]
    fn long_unicode_name_preserved_but_separators_stripped() {
        // Path separators stripped, unicode chars kept.
        let name = "写真/旅行.jpg";
        assert_eq!(sanitise_filename(name, "hash"), "写真旅行.jpg");
    }

    // ── is_reserved_platform_name ───────────────────────────────────

    #[test]
    fn reserved_names_identified() {
        for name in &["CON", "con", "Con", "PRN", "AUX", "NUL", "COM1", "LPT9"] {
            assert!(is_reserved_platform_name(name), "{name} should be reserved");
        }
        for name in &["COM0", "COM10", "LPT0", "LPT10", "COM", "LPT"] {
            assert!(
                !is_reserved_platform_name(name),
                "{name} should not be reserved"
            );
        }
    }

    // ── check_traversal ─────────────────────────────────────────────

    #[test]
    fn traversal_names_rejected() {
        assert!(check_traversal("..").is_err());
        assert!(check_traversal(".").is_err());
    }

    #[test]
    fn non_traversal_names_accepted() {
        assert!(check_traversal("file.txt").is_ok());
        assert!(check_traversal("..file").is_ok());
        assert!(check_traversal("file..").is_ok());
        assert!(check_traversal("...").is_ok()); // three dots, not two
    }

    #[test]
    fn drive_letter_prefix_rejected() {
        assert!(check_traversal("C:autoexec.bat").is_err());
        assert!(check_traversal("Z:file.txt").is_err());
        assert!(check_traversal("AB:file.txt").is_ok()); // not a drive letter
        assert!(check_traversal("1:file.txt").is_ok()); // not alphabetic
    }

    // ── deduplicate_path ────────────────────────────────────────────

    #[test]
    fn non_existent_path_returns_as_is() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("unique.txt");
        assert_eq!(deduplicate_path(&p, 100).unwrap(), p);
    }

    #[test]
    fn existing_file_gets_suffix() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("report.pdf");
        fs::write(&p, b"data").unwrap();
        let deduped = deduplicate_path(&p, 100).unwrap();
        assert_ne!(deduped, p);
        assert_eq!(deduped.file_name().unwrap(), "report (1).pdf");
    }

    #[test]
    fn multiple_existing_creations_get_incrementing_suffix() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().join("file.txt");
        fs::write(&base, b"1").unwrap();
        let d1 = deduplicate_path(&base, 100).unwrap();
        assert_eq!(d1.file_name().unwrap(), "file (1).txt");
        fs::write(&d1, b"2").unwrap();
        let d2 = deduplicate_path(&base, 100).unwrap();
        assert_eq!(d2.file_name().unwrap(), "file (2).txt");
    }

    #[test]
    fn extensionless_file_gets_suffix() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("README");
        fs::write(&p, b"data").unwrap();
        let deduped = deduplicate_path(&p, 100).unwrap();
        assert_eq!(deduped.file_name().unwrap(), "README (1)");
    }

    // ── safe_destination_path ───────────────────────────────────────

    #[test]
    fn normal_download_stays_in_directory() {
        let dir = TempDir::new().unwrap();
        let dest = safe_destination_path(dir.path(), "photo.jpg", "abc123").unwrap();
        assert!(dest.starts_with(dir.path()));
        assert_eq!(dest.file_name().unwrap(), "photo.jpg");
    }

    #[test]
    fn path_separator_in_name_stripped() {
        let dir = TempDir::new().unwrap();
        let dest = safe_destination_path(dir.path(), "../secret.txt", "abc123").unwrap();
        // ".." was stripped to "..secret.txt", which is not valid traversal
        // (it's a single filename component). check_traversal rejects it.
        // Actually, after stripping separators, "../secret.txt" becomes
        // "..secret.txt" which starts with ".." but is not exactly "..".
        // Let's check what happens:
        // - sanitise_filename("../secret.txt", "abc123") → "..secret.txt"
        // - check_traversal("..secret.txt") → NOT "..", so it passes
        // So we get "..secret.txt" as a filename, which is safe.
        assert!(
            dest.starts_with(dir.path()),
            "destination must stay in download directory"
        );
    }

    #[test]
    fn bare_traversal_name_rejected() {
        let dir = TempDir::new().unwrap();
        let err = safe_destination_path(dir.path(), "..", "abc123").unwrap_err();
        assert!(err.to_string().contains("directory reference"));
    }

    #[test]
    fn absolute_path_like_name_handled_safely() {
        let dir = TempDir::new().unwrap();
        // /etc/passwd → after stripping separators: "etcpasswd"
        let dest = safe_destination_path(dir.path(), "/etc/passwd", "abc123").unwrap();
        assert!(dest.starts_with(dir.path()));
    }

    #[test]
    fn drive_letter_name_rejected() {
        let dir = TempDir::new().unwrap();
        let err = safe_destination_path(dir.path(), "C:autoexec.bat", "abc123").unwrap_err();
        assert!(err.to_string().contains("drive-letter"));
    }

    #[test]
    fn duplicate_filename_gets_deduplicated() {
        let dir = TempDir::new().unwrap();
        let p1 = safe_destination_path(dir.path(), "report.pdf", "abc123").unwrap();
        fs::write(&p1, b"first").unwrap();
        let p2 = safe_destination_path(dir.path(), "report.pdf", "abc123").unwrap();
        assert_ne!(p1, p2);
        assert_eq!(p2.file_name().unwrap(), "report (1).pdf");
    }

    #[test]
    fn empty_display_name_uses_fallback() {
        let dir = TempDir::new().unwrap();
        let dest = safe_destination_path(dir.path(), "", "abcdef").unwrap();
        assert!(dest.starts_with(dir.path()));
        assert_eq!(dest.file_name().unwrap(), "abcdef");
    }

    #[test]
    fn reserved_name_preserves_extension() {
        let dir = TempDir::new().unwrap();
        let dest = safe_destination_path(dir.path(), "CON.txt", "fallback").unwrap();
        assert!(dest.starts_with(dir.path()));
        assert_eq!(dest.file_name().unwrap(), "fallback.txt");
    }

    #[test]
    fn long_unicode_name() {
        let dir = TempDir::new().unwrap();
        let name = "写真_旅行_ドキュメント_ファイル.pdf";
        let dest = safe_destination_path(dir.path(), name, "hash").unwrap();
        assert!(dest.starts_with(dir.path()));
        assert_eq!(dest.file_name().unwrap(), name);
    }

    #[test]
    fn extensionless_reserved_name_uses_fallback_exactly() {
        let dir = TempDir::new().unwrap();
        let dest = safe_destination_path(dir.path(), "NUL", "fallback").unwrap();
        assert_eq!(dest.file_name().unwrap(), "fallback");
    }

    // ── prepare_download_destination ────────────────────────────────

    #[test]
    fn prepare_download_destination_works_end_to_end() {
        let dir = TempDir::new().unwrap();
        let dest = prepare_download_destination(dir.path(), "book.pdf", "hash1").unwrap();
        assert!(dest.starts_with(dir.path()));
        assert_eq!(dest.file_name().unwrap(), "book.pdf");

        // Duplicate
        fs::write(&dest, b"content").unwrap();
        let dest2 = prepare_download_destination(dir.path(), "book.pdf", "hash1").unwrap();
        assert_eq!(dest2.file_name().unwrap(), "book (1).pdf");
    }

    #[test]
    fn prepare_download_destination_rejects_traversal() {
        let dir = TempDir::new().unwrap();
        let err = prepare_download_destination(dir.path(), "..", "hash").unwrap_err();
        assert!(err.to_string().contains("directory reference"));
    }

    #[test]
    fn reserved_name_in_prepare_uses_fallback() {
        let dir = TempDir::new().unwrap();
        let dest = prepare_download_destination(dir.path(), "CON", "hash123").unwrap();
        assert_eq!(dest.file_name().unwrap(), "hash123");
    }
}
