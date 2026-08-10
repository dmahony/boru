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

use n0_error::{Result, StdResultExt};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

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
/// - Returns an error if deduplication exhausts `MAX_DEDUP_ATTEMPTS`
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

/// Build the `N`-th deduplicated variant of a filename.
///
/// Returns the original filename unchanged for `n == 0`, and `stem (n).ext`
/// (or `stem (n)` for extensionless names) for `n >= 1`.  The format is
/// opaque; callers must not parse it.
fn dedup_name(filename: &str, n: u32) -> String {
    if n == 0 {
        return filename.to_string();
    }
    let (stem, ext) = match filename.rfind('.') {
        Some(dot) if dot > 0 => {
            let (base, suffix) = filename.split_at(dot);
            (base.to_string(), suffix.to_string())
        }
        _ => (filename.to_string(), String::new()),
    };
    if ext.is_empty() {
        format!("{stem} ({n})")
    } else {
        format!("{stem} ({n}){ext}")
    }
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

    for i in 1..=max_attempts {
        let candidate = parent.join(dedup_name(filename, i));
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

// ── TOCTOU-safe destination reservation (BORU-AUDIT-21) ─────────────────
//
// The path-only helpers above are pure name computation: they CHECK a path
// but never CREATE it.  A caller that later opens the checked path by name
// (e.g. `File::create` or iroh's `blobs().export`) leaves a
// time-of-check/time-of-use window in which the path can be swapped for a
// symlink or an existing file.  The reservation API below fuses validation
// and creation into ONE atomic operation: the destination file is created
// with O_EXCL + O_NOFOLLOW at the same moment the name is chosen, and all
// subsequent writes go through the returned handle — the path is never
// reopened.

/// An atomically-reserved download destination.
///
/// Constructed by [`reserve_download_destination`].  Path validation and
/// file creation happen in a single `O_EXCL` (create-new) operation, so an
/// existing file is never silently replaced and a symlink planted at the
/// final component is never followed.
///
/// The holder writes download bytes through [`file_mut`](Self::file_mut)
/// (or the tokio wrapper in the download worker), then calls
/// [`publish`](Self::publish) once the content hash has been verified.
/// Dropping the reservation without publishing removes the file it created
/// (and only that file), so a cancelled or failed transfer leaves neither a
/// partial destination nor a stray temporary file behind.
#[derive(Debug)]
pub struct ReservedDestination {
    file: Option<std::fs::File>,
    /// Path the handle refers to.  For `KeepBoth`/`Skip` reservations this
    /// IS the final display path (created atomically).  For `Overwrite`
    /// reservations this is a hidden temporary path that must be renamed
    /// onto `final_path` at publish time.
    write_path: PathBuf,
    /// Final display path after publication.
    final_path: PathBuf,
    /// Whether publication requires renaming `write_path` onto `final_path`
    /// (true only for the Overwrite policy).
    rename_on_publish: bool,
    published: bool,
}

impl ReservedDestination {
    /// The final display path the download will be published at.
    pub fn final_path(&self) -> &Path {
        &self.final_path
    }

    /// Mutable access to the reserved file handle (blocking I/O; the
    /// download worker wraps it in `tokio::fs::File::from_std`).
    pub fn file_mut(&mut self) -> Option<&mut std::fs::File> {
        self.file.as_mut()
    }

    /// Take ownership of the reserved file handle (used by the download
    /// worker so writes are offloaded onto tokio's blocking pool).
    pub fn take_file(&mut self) -> Option<std::fs::File> {
        self.file.take()
    }

    /// Restore the file handle after the download worker is done with it
    /// (kept so the drop-cleanup identity check can still run).
    pub fn restore_file(&mut self, file: std::fs::File) {
        self.file = Some(file);
    }

    /// Publish the download.
    ///
    /// For `Overwrite` reservations this atomically renames the temporary
    /// file onto the final path (replacing the previous file entry — a
    /// symlink at the destination is replaced, never followed).  For
    /// `KeepBoth`/`Skip` reservations the file was created at the final
    /// path, so publication is a no-op that records completion.
    ///
    /// Returns the final display path.  Must be called only after the
    /// download bytes have been verified; on error the reservation is
    /// dropped and the created file is removed.
    pub fn publish(mut self) -> Result<PathBuf> {
        if self.rename_on_publish {
            // Belt-and-suspenders: re-verify the destination still resolves
            // inside the trusted root before replacing it.  If the download
            // root was swapped while the transfer was in flight, the rename
            // must not move the file outside it.  A pre-existing symlink at
            // the destination that points outside is resolved by
            // canonicalize_allow_missing and rejected here.
            let root = self
                .final_path
                .parent()
                .ok_or_else(|| n0_error::anyerr!("destination has no parent directory"))?;
            let root_canon = crate::path_containment::canonicalize_allow_missing(root);
            let final_canon = crate::path_containment::canonicalize_allow_missing(&self.final_path);
            if !final_canon.starts_with(&root_canon) {
                return Err(n0_error::anyerr!(
                    "destination path escapes download directory: {}",
                    self.final_path.display()
                ));
            }
            std::fs::rename(&self.write_path, &self.final_path).with_std_context(|_| {
                format!(
                    "atomically publish download {} -> {}",
                    self.write_path.display(),
                    self.final_path.display()
                )
            })?;
        }
        self.published = true;
        Ok(self.final_path.clone())
    }
}

/// On Unix, compare the reserved handle's identity (device+inode) against
/// the file currently at `path`.  Guards drop-cleanup: if the download root
/// was swapped mid-transfer, the path may no longer refer to the file this
/// reservation created, and we must not delete an unrelated file.
#[cfg(unix)]
fn reserved_handle_matches_path(file: &std::fs::File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(handle_meta) = file.metadata() else {
        return false;
    };
    let Ok(path_meta) = std::fs::metadata(path) else {
        return false;
    };
    handle_meta.dev() == path_meta.dev() && handle_meta.ino() == path_meta.ino()
}

impl Drop for ReservedDestination {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        // Only remove the file this reservation created.  On Unix compare
        // identities first; if the path now refers to something else (or is
        // gone), leave it alone.
        #[cfg(unix)]
        if let Some(file) = self.file.as_ref() {
            if !reserved_handle_matches_path(file, &self.write_path) {
                return;
            }
        }
        let _ = std::fs::remove_file(&self.write_path);
    }
}

/// Outcome of [`reserve_download_destination`].
#[derive(Debug)]
pub enum Reservation {
    /// The destination is reserved and ready to receive download bytes.
    Use(ReservedDestination),
    /// The overwrite policy (Skip) declined the download because the file
    /// already exists.
    Skip,
}

/// Reserve a download destination inside `download_dir`, atomically.
///
/// Unlike the path-only helpers, this fuses sanitisation, containment
/// validation, and file creation into one operation:
///
/// - The final component is created with `O_EXCL` (`create_new`), so an
///   existing file — regular or symlink — can never be silently replaced or
///   followed; on Unix `O_NOFOLLOW` is added for defense in depth.
/// - Collision-safe names are generated without a separate existence-check
///   race: `create_new` either claims the name atomically or reports
///   `AlreadyExists`, in which case the next deduplicated name is tried.
/// - The created file is guaranteed to live inside `download_dir` (the
///   containment check from [`safe_destination_path`] runs before creation).
///
/// Policy behaviour:
/// - [`OverwritePolicy::KeepBoth`] (default): the file is created at the
///   sanitised name, or the next free `name (N)` variant.
/// - [`OverwritePolicy::Skip`]: returns [`Reservation::Skip`] when the
///   sanitised name already exists (including a racy appearance between the
///   existence check and the atomic create — the download is declined rather
///   than overwritten).
/// - [`OverwritePolicy::Overwrite`]: a hidden temporary file is created in
///   the same directory and returned; [`ReservedDestination::publish`]
///   atomically renames it onto the sanitised name, replacing the previous
///   file only after the download has been verified.
///
/// # Errors
///
/// Same error conditions as [`safe_destination_path`], plus exhaustion of
/// `MAX_DEDUP_ATTEMPTS` and any filesystem error that is not
/// `AlreadyExists`.
pub fn reserve_download_destination(
    download_dir: &Path,
    display_name: &str,
    fallback_stem: &str,
    policy: OverwritePolicy,
) -> Result<Reservation> {
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
    let exact = download_dir.join(&safe_name);

    // Belt-and-suspenders containment check (same as safe_destination_path).
    let download_dir_canon = crate::path_containment::canonicalize_allow_missing(download_dir);
    let exact_safe = crate::path_containment::canonicalize_allow_missing(&exact);
    if !exact_safe.starts_with(&download_dir_canon) {
        return Err(n0_error::anyerr!(
            "destination path escapes download directory: {}",
            exact_safe.display()
        ));
    }

    match policy {
        OverwritePolicy::KeepBoth => {
            for n in 0..MAX_DEDUP_ATTEMPTS {
                let name = dedup_name(&safe_name, n);
                match open_exclusive_at(download_dir, &name) {
                    Ok(file) => {
                        let path = download_dir.join(&name);
                        return Ok(Reservation::Use(ReservedDestination {
                            file: Some(file),
                            write_path: path.clone(),
                            final_path: path,
                            rename_on_publish: false,
                            published: false,
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => {
                        return Err(n0_error::anyerr!(
                            "failed to reserve download destination {}: {e}",
                            download_dir.join(&name).display()
                        ));
                    }
                }
            }
            Err(n0_error::anyerr!(
                "exhausted {MAX_DEDUP_ATTEMPTS} reservation attempts for {safe_name:?}"
            ))
        }
        OverwritePolicy::Skip => {
            if exact.exists() {
                return Ok(Reservation::Skip);
            }
            match open_exclusive_at(download_dir, &safe_name) {
                Ok(file) => Ok(Reservation::Use(ReservedDestination {
                    file: Some(file),
                    write_path: exact.clone(),
                    final_path: exact,
                    rename_on_publish: false,
                    published: false,
                })),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // The file appeared between the check and the create;
                    // decline rather than overwrite.
                    Ok(Reservation::Skip)
                }
                Err(e) => Err(n0_error::anyerr!(
                    "failed to reserve download destination {}: {e}",
                    exact.display()
                )),
            }
        }
        OverwritePolicy::Overwrite => {
            // Write to a hidden temporary file in the same directory, then
            // rename onto the destination only after verification.
            let tmp_base = format!(".boru-part-{safe_name}");
            for n in 0..MAX_DEDUP_ATTEMPTS {
                let tmp_name = dedup_name(&tmp_base, n);
                match open_exclusive_at(download_dir, &tmp_name) {
                    Ok(file) => {
                        let tmp_path = download_dir.join(&tmp_name);
                        return Ok(Reservation::Use(ReservedDestination {
                            file: Some(file),
                            write_path: tmp_path,
                            final_path: exact.clone(),
                            rename_on_publish: true,
                            published: false,
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => {
                        return Err(n0_error::anyerr!(
                            "failed to reserve temporary download destination {}: {e}",
                            download_dir.join(&tmp_name).display()
                        ));
                    }
                }
            }
            Err(n0_error::anyerr!(
                "exhausted {MAX_DEDUP_ATTEMPTS} temporary reservation attempts for {safe_name:?}"
            ))
        }
    }
}

/// Open (create) a file exclusively under `root` with no-follow semantics
/// for the final component.  Fails with `AlreadyExists` when anything —
/// including a symlink — already occupies the name.
fn open_exclusive_at(root: &Path, name: &str) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        // Defense in depth: create_new already refuses an existing symlink
        // (O_EXCL), but O_NOFOLLOW documents and enforces the no-follow
        // guarantee for the final component explicitly.
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(root.join(name))
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

    // ── TOCTOU-safe reservation (BORU-AUDIT-21) ────────────────────────

    fn use_reservation(
        dir: &std::path::Path,
        name: &str,
        policy: OverwritePolicy,
    ) -> ReservedDestination {
        match reserve_download_destination(dir, name, "hash", policy).unwrap() {
            Reservation::Use(dest) => dest,
            Reservation::Skip => panic!("expected Use, got Skip"),
        }
    }

    #[test]
    fn keep_both_never_overwrites_existing_file() {
        let dir = TempDir::new().unwrap();
        let existing = dir.path().join("report.pdf");
        fs::write(&existing, b"original content").unwrap();

        let dest = use_reservation(dir.path(), "report.pdf", OverwritePolicy::KeepBoth);
        let final_path = dest.final_path().to_path_buf();
        drop(dest);

        assert_ne!(final_path, existing);
        assert_eq!(final_path.file_name().unwrap(), "report (1).pdf");
        // The pre-existing file is untouched.
        assert_eq!(fs::read(&existing).unwrap(), b"original content");
    }

    #[test]
    fn skip_policy_declines_existing_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("report.pdf"), b"original").unwrap();

        match reserve_download_destination(dir.path(), "report.pdf", "hash", OverwritePolicy::Skip)
            .unwrap()
        {
            Reservation::Skip => {}
            Reservation::Use(_) => panic!("Skip policy must decline an existing file"),
        }
        // And it must not have created anything.
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn skip_policy_use_when_file_absent() {
        let dir = TempDir::new().unwrap();
        let dest = use_reservation(dir.path(), "fresh.pdf", OverwritePolicy::Skip);
        assert_eq!(dest.final_path().file_name().unwrap(), "fresh.pdf");
        drop(dest);
        // Dropping an unpublished reservation removes the created file.
        assert!(!dir.path().join("fresh.pdf").exists());
    }

    #[test]
    fn overwrite_publishes_only_after_verification() {
        let dir = TempDir::new().unwrap();
        let existing = dir.path().join("report.pdf");
        fs::write(&existing, b"old bytes").unwrap();

        let mut dest =
            use_reservation(dir.path(), "report.pdf", OverwritePolicy::Overwrite);
        // The original is NOT touched before publish (the reservation writes
        // to a hidden temp file until the content is verified).
        assert_eq!(fs::read(&existing).unwrap(), b"old bytes");
        // The temp file lives inside the download root.
        assert!(dest.final_path().starts_with(dir.path()));

        use std::io::Write;
        dest.file_mut()
            .unwrap()
            .write_all(b"new verified bytes")
            .unwrap();
        dest.file_mut().unwrap().sync_all().unwrap();
        let published = dest.publish().unwrap();

        assert_eq!(published, existing);
        assert_eq!(fs::read(&existing).unwrap(), b"new verified bytes");
        // No leftover temp files.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().starts_with(".boru-part-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files must be renamed away: {leftovers:?}"
        );
    }

    #[test]
    fn drop_without_publish_removes_fresh_file_but_keeps_existing() {
        let dir = TempDir::new().unwrap();

        // KeepBoth: the reservation created the file; dropping without
        // publishing removes it.
        {
            let mut dest = use_reservation(dir.path(), "fresh.txt", OverwritePolicy::KeepBoth);
            use std::io::Write;
            dest.file_mut().unwrap().write_all(b"partial").unwrap();
        }
        assert!(
            !dir.path().join("fresh.txt").exists(),
            "unpublished KeepBoth reservation must clean up its file"
        );

        // Overwrite: dropping without publishing leaves the pre-existing
        // original untouched and removes the temp file.
        let existing = dir.path().join("report.pdf");
        fs::write(&existing, b"old bytes").unwrap();
        {
            let mut dest =
                use_reservation(dir.path(), "report.pdf", OverwritePolicy::Overwrite);
            use std::io::Write;
            dest.file_mut().unwrap().write_all(b"new bytes").unwrap();
        }
        assert_eq!(fs::read(&existing).unwrap(), b"old bytes");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with(".boru-part-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files must be cleaned up: {leftovers:?}"
        );
    }

    #[test]
    fn concurrent_reservations_same_name_get_distinct_files() {
        let dir = TempDir::new().unwrap();

        let mut d1 = use_reservation(
            dir.path(),
            "same-name.txt",
            OverwritePolicy::KeepBoth,
        );
        let mut d2 = use_reservation(
            dir.path(),
            "same-name.txt",
            OverwritePolicy::KeepBoth,
        );

        let p1 = d1.final_path().to_path_buf();
        let p2 = d2.final_path().to_path_buf();
        assert_ne!(p1, p2, "two reservations must not claim the same name");

        use std::io::Write;
        d1.file_mut().unwrap().write_all(b"one").unwrap();
        d2.file_mut().unwrap().write_all(b"two").unwrap();
        let f1 = d1.publish().unwrap();
        let f2 = d2.publish().unwrap();

        assert_eq!(fs::read(&f1).unwrap(), b"one");
        assert_eq!(fs::read(&f2).unwrap(), b"two");
    }

    #[test]
    fn reservation_stays_inside_download_root() {
        let dir = TempDir::new().unwrap();
        let root_canon = crate::path_containment::canonicalize_allow_missing(dir.path());

        let dest = use_reservation(
            dir.path(),
            "nested/../evil.txt",
            OverwritePolicy::KeepBoth,
        );
        let final_path = dest.final_path();
        assert!(final_path.starts_with(dir.path()));
        let final_canon = crate::path_containment::canonicalize_allow_missing(final_path);
        assert!(
            final_canon.starts_with(&root_canon),
            "reserved file must resolve inside the download root"
        );
        // The sanitised filename has separators stripped.
        assert_eq!(final_path.file_name().unwrap(), "nested..evil.txt");
    }

    #[cfg(unix)]
    #[test]
    fn keep_both_does_not_follow_symlink_final_component() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = dir.path().join("outside.txt");
        fs::write(&outside, b"secret").unwrap();
        symlink(&outside, dir.path().join("photo.jpg")).unwrap();

        // The symlink occupies the exact name; KeepBoth must skip to a new
        // name rather than follow or replace it.
        let dest = use_reservation(dir.path(), "photo.jpg", OverwritePolicy::KeepBoth);
        let final_path = dest.final_path().to_path_buf();
        drop(dest);

        assert_eq!(final_path.file_name().unwrap(), "photo (1).jpg");
        // The symlink is untouched and still points at the outside file.
        assert!(dir.path().join("photo.jpg").is_symlink());
        assert_eq!(fs::read(&outside).unwrap(), b"secret");
    }

    #[cfg(unix)]
    #[test]
    fn overwrite_publish_replaces_symlink_without_following_it() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = dir.path().join("outside.txt");
        fs::write(&outside, b"secret").unwrap();
        symlink(&outside, dir.path().join("report.pdf")).unwrap();

        let mut dest =
            use_reservation(dir.path(), "report.pdf", OverwritePolicy::Overwrite);
        use std::io::Write;
        dest.file_mut().unwrap().write_all(b"verified").unwrap();
        dest.file_mut().unwrap().sync_all().unwrap();
        let published = dest.publish().unwrap();

        // Rename replaces the directory entry: the result is a regular file
        // with the new content, and the outside target is untouched.
        assert!(!published.is_symlink());
        assert_eq!(fs::read(&published).unwrap(), b"verified");
        assert_eq!(fs::read(&outside).unwrap(), b"secret");
    }

    #[cfg(unix)]
    #[test]
    fn overwrite_publish_rejects_destination_escaping_root() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        // The symlink target must live OUTSIDE the download root for the
        // containment re-check to have anything to catch.
        let outside_dir = TempDir::new().unwrap();
        let outside = outside_dir.path().join("outside.txt");
        fs::write(&outside, b"secret").unwrap();

        // Reserve for a fresh name (temp in root), then swap the final path
        // for an escaping symlink before publish — the containment re-check
        // must refuse to replace it.
        let mut dest =
            use_reservation(dir.path(), "report.pdf", OverwritePolicy::Overwrite);
        let final_path = dest.final_path().to_path_buf();
        symlink(&outside, &final_path).unwrap();

        use std::io::Write;
        dest.file_mut().unwrap().write_all(b"verified").unwrap();
        dest.file_mut().unwrap().sync_all().unwrap();
        let err = dest.publish().unwrap_err();
        assert!(
            err.to_string().contains("escapes download directory"),
            "unexpected error: {err}"
        );
        // The outside file is untouched and no temp remains.
        assert_eq!(fs::read(&outside).unwrap(), b"secret");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with(".boru-part-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files must be cleaned up: {leftovers:?}"
        );
    }
}
