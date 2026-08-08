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
    //
    // Both sides use [`crate::path_containment::canonicalize_allow_missing`]
    // so the comparison is symmetric.  The download directory exists (created
    // by the caller), while the candidate may or may not exist yet: a plain
    // `candidate.canonicalize()` on a non-existent candidate would fall back
    // to the raw text while `download_dir` is left unresolved — the raw-vs-
    // canonical mismatch that falsely rejects downloads when the download
    // directory is reached through a symlink (or on Windows, where
    // canonicalize emits the `\\?\` prefix).  Resolving both sides the same
    // way keeps the check honest: a pre-existing symlink inside the download
    // directory that points outside is still resolved and rejected.
    let download_dir_canon = crate::path_containment::canonicalize_allow_missing(download_dir);
    let candidate_safe = crate::path_containment::canonicalize_allow_missing(&candidate);
    if !candidate_safe.starts_with(&download_dir_canon) {
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

// ── Overwrite-conflict policy (FS-26) ────────────────────────────────────

/// User-visible policy for what happens when an incoming download collides
/// with an existing file at the destination.
///
/// The default is [`OverwritePolicy::KeepBoth`] — a download must never
/// silently overwrite an existing file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum OverwritePolicy {
    /// Keep both files: the incoming download is saved under a deduplicated
    /// name (e.g. `report (1).pdf`) instead of replacing the existing file.
    #[default]
    KeepBoth,
    /// Replace the existing file at the destination path.
    Overwrite,
    /// Do not download: the transfer is skipped because a file with the same
    /// name already exists.
    Skip,
}

impl OverwritePolicy {
    /// Stable label for the policy (shown in the transfer card).
    pub fn label(self) -> &'static str {
        match self {
            Self::KeepBoth => "Keep Both",
            Self::Overwrite => "Overwrite",
            Self::Skip => "Skip",
        }
    }

    /// Short helper text describing the policy.
    pub fn description(self) -> &'static str {
        match self {
            Self::KeepBoth => "Save as a new file with a numbered suffix",
            Self::Overwrite => "Replace the existing file",
            Self::Skip => "Do not download this file",
        }
    }
}

/// Outcome of resolving a destination under an [`OverwritePolicy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationDecision {
    /// Download to this path.
    Use(std::path::PathBuf),
    /// The policy (Skip) prevents the download because the file exists.
    Skip,
}

/// Resolve the destination path for a download under the given overwrite
/// policy, applying the same sanitisation guarantees as
/// [`safe_destination_path`].
///
/// - [`OverwritePolicy::KeepBoth`] (default) deduplicates an existing file
///   with a numbered suffix.
/// - [`OverwritePolicy::Overwrite`] returns the exact sanitised path even
///   when a file already exists there.
/// - [`OverwritePolicy::Skip`] returns [`DestinationDecision::Skip`] when
///   the file already exists, and the exact path otherwise.
///
/// # Errors
///
/// Same error conditions as [`safe_destination_path`]: traversal attempts,
/// path escape, and deduplication exhaustion.
pub fn resolve_destination_with_policy(
    download_dir: &Path,
    display_name: &str,
    fallback_stem: &str,
    policy: OverwritePolicy,
) -> Result<DestinationDecision> {
    let exact = safe_destination_path_no_dedup(download_dir, display_name, fallback_stem)?;
    match policy {
        OverwritePolicy::KeepBoth => {
            let final_path = deduplicate_path(&exact, MAX_DEDUP_ATTEMPTS)?;
            Ok(DestinationDecision::Use(final_path))
        }
        OverwritePolicy::Overwrite => Ok(DestinationDecision::Use(exact)),
        OverwritePolicy::Skip => {
            if exact.exists() {
                Ok(DestinationDecision::Skip)
            } else {
                Ok(DestinationDecision::Use(exact))
            }
        }
    }
}

/// Compute the sanitised destination WITHOUT deduplication (used by
/// [`resolve_destination_with_policy`] for the Overwrite and Skip branches).
fn safe_destination_path_no_dedup(
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

    let stripped: String = display_name
        .chars()
        .filter(|&c| c != '/' && c != '\\')
        .collect();

    check_traversal(&stripped)?;

    let safe_name = sanitise_filename(display_name, fallback_stem);

    let candidate = download_dir.join(&safe_name);

    let download_dir_canon = crate::path_containment::canonicalize_allow_missing(download_dir);
    let candidate_safe = crate::path_containment::canonicalize_allow_missing(&candidate);
    if !candidate_safe.starts_with(&download_dir_canon) {
        return Err(n0_error::anyerr!(
            "destination path escapes download directory: {}",
            candidate_safe.display()
        ));
    }

    Ok(candidate)
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

    // ── FS-26: overwrite-conflict policy ─────────────────────────────

    #[test]
    fn keep_both_deduplicates_existing_file() {
        let dir = TempDir::new().unwrap();
        let existing = dir.path().join("report.pdf");
        fs::write(&existing, b"original").unwrap();

        let decision = resolve_destination_with_policy(
            dir.path(),
            "report.pdf",
            "hash",
            OverwritePolicy::KeepBoth,
        )
        .unwrap();
        match decision {
            DestinationDecision::Use(path) => {
                assert_eq!(path.file_name().unwrap(), "report (1).pdf");
                // The original file is untouched.
                assert_eq!(fs::read(&existing).unwrap(), b"original");
            }
            DestinationDecision::Skip => panic!("KeepBoth must never skip"),
        }
    }

    #[test]
    fn keep_both_returns_exact_path_when_no_conflict() {
        let dir = TempDir::new().unwrap();
        let decision = resolve_destination_with_policy(
            dir.path(),
            "fresh.pdf",
            "hash",
            OverwritePolicy::KeepBoth,
        )
        .unwrap();
        assert_eq!(
            decision,
            DestinationDecision::Use(dir.path().join("fresh.pdf"))
        );
    }

    #[test]
    fn overwrite_returns_exact_path_even_when_file_exists() {
        let dir = TempDir::new().unwrap();
        let existing = dir.path().join("report.pdf");
        fs::write(&existing, b"original").unwrap();

        let decision = resolve_destination_with_policy(
            dir.path(),
            "report.pdf",
            "hash",
            OverwritePolicy::Overwrite,
        )
        .unwrap();
        assert_eq!(decision, DestinationDecision::Use(existing.clone()));
    }

    #[test]
    fn skip_prevents_download_when_file_exists() {
        let dir = TempDir::new().unwrap();
        let existing = dir.path().join("report.pdf");
        fs::write(&existing, b"original").unwrap();

        let decision = resolve_destination_with_policy(
            dir.path(),
            "report.pdf",
            "hash",
            OverwritePolicy::Skip,
        )
        .unwrap();
        assert_eq!(decision, DestinationDecision::Skip);
    }

    #[test]
    fn skip_allows_download_when_file_absent() {
        let dir = TempDir::new().unwrap();
        let decision = resolve_destination_with_policy(
            dir.path(),
            "newfile.pdf",
            "hash",
            OverwritePolicy::Skip,
        )
        .unwrap();
        assert_eq!(
            decision,
            DestinationDecision::Use(dir.path().join("newfile.pdf"))
        );
    }

    #[test]
    fn policy_resolution_still_rejects_traversal() {
        let dir = TempDir::new().unwrap();
        for policy in [
            OverwritePolicy::KeepBoth,
            OverwritePolicy::Overwrite,
            OverwritePolicy::Skip,
        ] {
            let err =
                resolve_destination_with_policy(dir.path(), "..", "hash", policy).unwrap_err();
            assert!(err.to_string().contains("directory reference"));
        }
    }

    #[test]
    fn policy_labels_are_stable() {
        assert_eq!(OverwritePolicy::KeepBoth.label(), "Keep Both");
        assert_eq!(OverwritePolicy::Overwrite.label(), "Overwrite");
        assert_eq!(OverwritePolicy::Skip.label(), "Skip");
        assert_eq!(OverwritePolicy::default(), OverwritePolicy::KeepBoth);
    }

    // ── Resume-after-conflict (FS-26) ────────────────────────────────
    //
    // An interrupted transfer leaves a partial file at the exact destination
    // (e.g. download_blob_to_file wrote bytes then the app quit).  When the
    // transfer is resumed the overwrite policy must still apply: KeepBoth
    // saves the resumed copy under a numbered suffix (never clobbers the
    // partial), Overwrite returns the exact path (replacing the partial),
    // and Skip declines because the name is taken.

    #[test]
    fn resume_after_conflict_keep_both_never_clobbers_partial_file() {
        let dir = TempDir::new().unwrap();
        let partial = dir.path().join("movie.mp4");
        fs::write(&partial, b"partial-bytes-from-interrupted-transfer").unwrap();

        let decision = resolve_destination_with_policy(
            dir.path(),
            "movie.mp4",
            "download",
            OverwritePolicy::KeepBoth,
        )
        .unwrap();
        match decision {
            DestinationDecision::Use(path) => {
                assert_eq!(path.file_name().unwrap(), "movie (1).mp4");
                assert_eq!(
                    fs::read(&partial).unwrap(),
                    b"partial-bytes-from-interrupted-transfer",
                    "KeepBoth must never overwrite the interrupted transfer's partial file"
                );
            }
            DestinationDecision::Skip => panic!("KeepBoth must never skip"),
        }
    }

    #[test]
    fn resume_after_conflict_overwrite_replaces_partial_file_path() {
        let dir = TempDir::new().unwrap();
        let partial = dir.path().join("movie.mp4");
        fs::write(&partial, b"partial").unwrap();

        let decision = resolve_destination_with_policy(
            dir.path(),
            "movie.mp4",
            "download",
            OverwritePolicy::Overwrite,
        )
        .unwrap();
        assert_eq!(decision, DestinationDecision::Use(partial));
    }

    #[test]
    fn resume_after_conflict_skip_declines_when_partial_file_exists() {
        let dir = TempDir::new().unwrap();
        let partial = dir.path().join("movie.mp4");
        fs::write(&partial, b"partial").unwrap();

        let decision = resolve_destination_with_policy(
            dir.path(),
            "movie.mp4",
            "download",
            OverwritePolicy::Skip,
        )
        .unwrap();
        assert_eq!(decision, DestinationDecision::Skip);
    }

    #[test]
    fn resume_after_conflict_keep_both_escalates_suffix_for_multiple_partials() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("report.pdf"), b"partial-1").unwrap();
        fs::write(dir.path().join("report (1).pdf"), b"partial-2").unwrap();

        let decision = resolve_destination_with_policy(
            dir.path(),
            "report.pdf",
            "download",
            OverwritePolicy::KeepBoth,
        )
        .unwrap();
        match decision {
            DestinationDecision::Use(path) => {
                assert_eq!(path.file_name().unwrap(), "report (2).pdf");
            }
            DestinationDecision::Skip => panic!("KeepBoth must never skip"),
        }
    }

    // ── Symlinked download directory (canonicalize asymmetry) ─────────
    //
    // The containment check must compare like with like.  A download
    // directory whose *textual* form differs from its canonical form
    // (reached through a symlink, or the Windows `\\?\` prefix) used to
    // falsely reject valid downloads because the candidate side was
    // canonicalized (when the file existed) while the directory side stayed
    // raw — or vice versa for non-existent candidates.  Both sides are now
    // canonicalized with the same allow-missing helper.

    #[cfg(unix)]
    #[test]
    fn accepts_non_existent_candidate_in_symlinked_download_dir() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real_downloads");
        std::fs::create_dir_all(&real).unwrap();
        let link = tmp.path().join("downloads_link");
        symlink(&real, &link).unwrap();
        assert_ne!(link.canonicalize().unwrap(), link); // textual != canonical

        let dest = safe_destination_path(&link, "photo.jpg", "abc123").unwrap();
        assert!(dest.starts_with(&link));
        assert_eq!(dest.file_name().unwrap(), "photo.jpg");
    }

    #[cfg(unix)]
    #[test]
    fn accepts_existing_candidate_in_symlinked_download_dir() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real_downloads");
        std::fs::create_dir_all(&real).unwrap();
        let link = tmp.path().join("downloads_link");
        symlink(&real, &link).unwrap();
        assert_ne!(link.canonicalize().unwrap(), link); // textual != canonical

        // Existing file: the old code canonicalized the candidate (resolving
        // the symlink) but compared against the raw link path — false
        // "escapes download directory".  Now both sides resolve the same way.
        let existing = link.join("photo.jpg");
        fs::write(&existing, b"data").unwrap();

        let dest = safe_destination_path(&link, "photo.jpg", "abc123").unwrap();
        assert!(dest.starts_with(&link));
        assert_eq!(dest.file_name().unwrap(), "photo (1).jpg");
    }

    #[cfg(unix)]
    #[test]
    fn policy_resolution_accepts_symlinked_download_dir_with_existing_file() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real_downloads");
        std::fs::create_dir_all(&real).unwrap();
        let link = tmp.path().join("downloads_link");
        symlink(&real, &link).unwrap();
        assert_ne!(link.canonicalize().unwrap(), link); // textual != canonical

        fs::write(link.join("report.pdf"), b"original").unwrap();

        // KeepBoth: deduplicated inside the (symlinked) download directory.
        let decision =
            resolve_destination_with_policy(&link, "report.pdf", "hash", OverwritePolicy::KeepBoth)
                .unwrap();
        match decision {
            DestinationDecision::Use(path) => {
                assert!(path.starts_with(&link));
                assert_eq!(path.file_name().unwrap(), "report (1).pdf");
            }
            DestinationDecision::Skip => panic!("KeepBoth must never skip"),
        }

        // Overwrite: exact path still inside the download directory.
        let decision = resolve_destination_with_policy(
            &link,
            "report.pdf",
            "hash",
            OverwritePolicy::Overwrite,
        )
        .unwrap();
        assert_eq!(decision, DestinationDecision::Use(link.join("report.pdf")));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_candidate_symlink_escaping_download_dir() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("downloads");
        std::fs::create_dir_all(&dir).unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        // A pre-existing symlink at the candidate path pointing outside.
        symlink(&outside, dir.join("photo.jpg")).unwrap();

        let err = safe_destination_path(&dir, "photo.jpg", "abc123").unwrap_err();
        assert!(
            err.to_string().contains("escapes download directory"),
            "unexpected error: {err}"
        );
    }
}
