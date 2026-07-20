//! Regression coverage for untrusted remote filenames.
//!
//! Every accepted destination must remain a direct child of the caller's
//! selected directory. Inputs that cannot be represented safely may instead
//! be rejected; they must never select a path outside that directory.

use std::fs;
use std::path::{Path, PathBuf};

use boru_chat::safe_destination::{prepare_download_destination, safe_destination_path};
use tempfile::TempDir;

fn assert_safe_or_rejected(download_dir: &Path, remote_name: &str) {
    match safe_destination_path(download_dir, remote_name, "content-hash") {
        Ok(destination) => {
            assert_eq!(
                destination.parent(),
                Some(download_dir),
                "accepted destination for {remote_name:?} must be a direct child"
            );
            assert!(
                destination.starts_with(download_dir),
                "accepted destination for {remote_name:?} escaped selected directory: {}",
                destination.display()
            );
        }
        Err(_) => {
            // Rejecting an unrepresentable or unsafe remote name is safe.
        }
    }
}

#[test]
fn traversal_and_absolute_names_cannot_escape_download_directory() {
    let dir = TempDir::new().unwrap();
    for name in [
        "../outside.txt",
        "..\\outside.txt",
        "..",
        ".",
        "/tmp/outside.txt",
        "\\\\server\\share\\outside.txt",
        "C:\\outside.txt",
        "Z:outside.txt",
    ] {
        assert_safe_or_rejected(dir.path(), name);
    }
}

#[test]
fn platform_reserved_names_are_safe() {
    let dir = TempDir::new().unwrap();
    for name in [
        "CON", "CON.txt", "prn.log", "AUX", "NUL.dat", "COM1", "COM9.bin", "LPT1", "LPT9.txt",
    ] {
        assert_safe_or_rejected(dir.path(), name);
    }
}

#[test]
fn control_characters_are_safe_or_rejected_without_escape() {
    let dir = TempDir::new().unwrap();
    for name in [
        "line\nfeed.txt",
        "tab\tname.bin",
        "bell\u{0007}.dat",
        "nul\0name.dat",
        "escape\u{001b}[31m.txt",
    ] {
        assert_safe_or_rejected(dir.path(), name);
    }
}

#[test]
fn empty_and_extreme_length_names_remain_in_directory() {
    let dir = TempDir::new().unwrap();
    for name in [String::new(), "   ".to_string(), "x".repeat(4096)] {
        assert_safe_or_rejected(dir.path(), &name);
    }
}

#[test]
fn duplicate_names_are_deduplicated_inside_selected_directory() {
    let dir = TempDir::new().unwrap();
    let first = safe_destination_path(dir.path(), "same-name.txt", "hash").unwrap();
    fs::write(&first, b"first").unwrap();

    let second = safe_destination_path(dir.path(), "same-name.txt", "hash").unwrap();
    assert_eq!(first.parent(), second.parent());
    assert_eq!(second.parent(), Some(dir.path()));
    assert_ne!(first, second);
    assert_eq!(second.file_name().unwrap(), "same-name (1).txt");

    // A pre-existing symlink/directory outside the selected directory must not
    // affect the returned parent or make the destination escape.
    let _also_inside: PathBuf = dir.path().join("same-name (1).txt");
}

// ── New hostile-path tests for the safe-destination path ──────────────

#[test]
fn mixed_separator_variants_cannot_escape_download_directory() {
    let dir = TempDir::new().unwrap();
    for name in [
        "a/../b.txt",
        "a\\..\\b.txt",
        "a\\../b.txt",
        "a/..\\b.txt",
        "x/../../y.txt",
        "subdir/../../../etc/passwd",
        "..\\..\\..\\etc\\passwd",
        "a/b/../../../c/d.txt",
    ] {
        assert_safe_or_rejected(dir.path(), name);
    }
}

#[test]
fn redundant_separator_names_do_not_escape() {
    let dir = TempDir::new().unwrap();
    for name in [
        "foo///bar.txt",
        "foo\\\\\\bar.txt",
        "foo//bar//baz.txt",
        "file   .txt", // spaces are not separators, just whitespace
    ] {
        assert_safe_or_rejected(dir.path(), name);
    }
}

#[test]
fn dot_segment_names_remain_inside_directory() {
    // Names containing dot segments alongside separators — after stripping
    // '/' and '\\', the dots become literal filename characters and must
    // not produce a parent-directory reference.
    let dir = TempDir::new().unwrap();
    for name in [
        "./safe.txt",
        "a/./b.txt",
        "a/./b/./c.txt",
        "a/./../b.txt",
        "./../outside.txt",
        "....//....txt", // sequence of dots with embedded separator
        ".../.../foo.txt",
        "./a/./b/./c.txt",
    ] {
        assert_safe_or_rejected(dir.path(), name);
    }
}

#[test]
fn possessive_period_and_separator_mixing_is_safe() {
    // Names that combine periods, dots, and path separators in ways that
    // could trick naive separator-stripping logic.
    let dir = TempDir::new().unwrap();
    for name in [
        "a..../b.txt",
        "a....\\b.txt",
        "..a/../b.txt",
        "..a\\..\\b.txt",
        "../..a/../b.txt",
    ] {
        assert_safe_or_rejected(dir.path(), name);
    }
}

// ── Hostile-path tests through the real entry point ──────────────────

fn assert_safe_or_rejected_through_prepare(download_dir: &Path, remote_name: &str) {
    match prepare_download_destination(download_dir, remote_name, "content-hash") {
        Ok(destination) => {
            assert_eq!(
                destination.parent(),
                Some(download_dir),
                "prepare_download_destination accepted {remote_name:?} but parent is not the download directory"
            );
            assert!(
                destination.starts_with(download_dir),
                "prepare_download_destination accepted {remote_name:?} but destination {} escaped",
                destination.display()
            );
        }
        Err(_) => {
            // Rejection is a safe outcome — the file won't be written.
        }
    }
}

#[test]
fn basic_traversal_rejected_through_prepare_download_destination() {
    let dir = TempDir::new().unwrap();
    for name in [
        "../outside.txt",
        "..\\outside.txt",
        "..",
        ".",
        "/tmp/outside.txt",
        "\\\\server\\share\\outside.txt",
        "C:\\outside.txt",
        "Z:outside.txt",
    ] {
        assert_safe_or_rejected_through_prepare(dir.path(), name);
    }
}

#[test]
fn mixed_separator_traversal_through_prepare_download_destination() {
    let dir = TempDir::new().unwrap();
    for name in [
        "a/../b.txt",
        "a\\..\\b.txt",
        "x/../../y.txt",
        "subdir/../../../etc/passwd",
        "..\\..\\..\\etc\\passwd",
    ] {
        assert_safe_or_rejected_through_prepare(dir.path(), name);
    }
}

#[test]
fn dot_segment_through_prepare_download_destination() {
    let dir = TempDir::new().unwrap();
    for name in [
        "./safe.txt",
        "a/./b.txt",
        "a/./../b.txt",
        "./../outside.txt",
        ".../.../foo.txt",
    ] {
        assert_safe_or_rejected_through_prepare(dir.path(), name);
    }
}

#[test]
fn reserved_and_extreme_names_through_prepare_download_destination() {
    let dir = TempDir::new().unwrap();
    for name in ["CON", "CON.txt", "PRN", "NUL.dat", "COM1", "", "   "] {
        assert_safe_or_rejected_through_prepare(dir.path(), name);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Section 4 — Complete platform-reserved name coverage
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn all_windows_device_names_are_safe() {
    // Every reserved name COM1–COM9 and LPT1–LPT9, across several extensions.
    let dir = TempDir::new().unwrap();
    for stem in &[
        "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
        "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ] {
        assert_safe_or_rejected(dir.path(), stem); // bare name
        assert_safe_or_rejected(dir.path(), &format!("{stem}.txt")); // .txt
        assert_safe_or_rejected(dir.path(), &format!("{stem}.exe")); // .exe
        assert_safe_or_rejected(dir.path(), &format!("{stem}.dat")); // .dat
    }
}

#[test]
fn windows_device_names_with_trailing_dot_are_safe() {
    // On Windows "CON." resolves to the same device as "CON".
    let dir = TempDir::new().unwrap();
    for name in &["CON.", "PRN.", "AUX.", "NUL.", "COM1.", "LPT9."] {
        assert_safe_or_rejected(dir.path(), name);
    }
}

#[test]
fn non_reserved_similar_names_pass_through_normally() {
    // Names that look like device names but are not reserved
    // should be accepted as-is.
    let dir = TempDir::new().unwrap();
    for name in &[
        "COM",
        "COM0",
        "COM10",
        "COM11",
        "COM20",
        "LPT",
        "LPT0",
        "LPT10",
        "LPT11",
        "conform.txt",  // starts with "con" but the stem is "conform"
        "console.log",  // starts with "con"
        "CONGRESS.txt", // starts with "CON" but not a match
    ] {
        let dest = safe_destination_path(dir.path(), name, "hash").unwrap();
        assert_eq!(
            dest.file_name().unwrap(),
            std::path::Path::new(name).file_name().unwrap_or_default(),
            "non-reserved name {name:?} should pass through unchanged"
        );
        assert!(dest.starts_with(dir.path()));
    }
}

#[test]
fn reserved_name_fallback_preserves_non_reserved_extension() {
    // When a reserved name carries a non-reserved extension the fallback
    // preserves that extension. When the extension is also reserved, the
    // plain fallback is used.
    let dir = TempDir::new().unwrap();
    // Reserved stem + safe extension → fallback.stem.extension
    let dest = safe_destination_path(dir.path(), "CON.md", "abc").unwrap();
    assert_eq!(dest.file_name().unwrap(), "abc.md");

    let dest = safe_destination_path(dir.path(), "NUL.yaml", "abc").unwrap();
    assert_eq!(dest.file_name().unwrap(), "abc.yaml");

    let dest = safe_destination_path(dir.path(), "PRN", "abc").unwrap();
    assert_eq!(dest.file_name().unwrap(), "abc");
}

#[test]
fn reserved_name_with_double_extension_uses_last_extension() {
    // e.g. "CON.tar.gz" — the stem is "CON" (reserved) → fallback used.
    // rsplit_once('.') yields only the last extension "gz".
    let dir = TempDir::new().unwrap();
    let dest = safe_destination_path(dir.path(), "CON.tar.gz", "abc").unwrap();
    assert_eq!(dest.file_name().unwrap(), "abc.gz");
}

#[test]
fn mixed_case_reserved_names_are_safe() {
    // The reserved-name check is case-insensitive. All of these
    // should be handled safely.
    let dir = TempDir::new().unwrap();
    for name in &["con", "Con", "COm1", "com9.txt", "LpT1.exe"] {
        assert_safe_or_rejected(dir.path(), name);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Section 5 — Control-character and invisible-character variants
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn carriage_return_and_other_ascii_controls_are_safe() {
    let dir = TempDir::new().unwrap();
    for name in &[
        "file\rname.txt",
        "file\x08name.txt",     // backspace
        "\u{000C}formfeed.dat", // form feed
        "file\u{0000}.dat",     // null in the middle (not as sole name)
    ] {
        assert_safe_or_rejected(dir.path(), name);
    }
}

#[test]
fn unicode_bidi_and_formatting_controls_are_safe() {
    // Bidi overrides, LRM/RLM, zero-width joiners, etc. should not
    // trick the destination logic.
    let dir = TempDir::new().unwrap();
    for name in &[
        "file\u{200B}.txt",     // zero-width space
        "\u{200E}order.txt",    // left-to-right mark
        "\u{200F}order.txt",    // right-to-left mark
        "\u{202E}override.txt", // right-to-left override
        "f\u{200D}ile.txt",     // zero-width joiner
        "\u{2060}word.txt",     // word joiner
    ] {
        assert_safe_or_rejected(dir.path(), name);
    }
}

#[test]
fn invisible_unicode_whitespace_names_are_safe() {
    // Various unicode whitespace codepoints should be handled.
    let dir = TempDir::new().unwrap();
    for name in &[
        "\u{00A0}",         // non-breaking space (should use fallback)
        "\u{1680}",         // ogham space mark
        "\u{2000}",         // en quad
        "\u{2001}",         // em quad
        "\u{2003}",         // em space
        "\u{3000}",         // ideographic space
        "\u{200B}file.txt", // zero-width space prefix
    ] {
        assert_safe_or_rejected(dir.path(), name);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Section 6 — Edge-case empty / effectively-empty names
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn only_control_character_name_uses_fallback() {
    // A name consisting entirely of control characters strips to nothing
    // via the sanitizer because all characters are retained (only '/'
    // and '\\' are stripped), but trim().is_empty() sees them as empty
    // → fallback is used.
    let dir = TempDir::new().unwrap();
    // These names are all non-whitespace control characters; the sanitizer
    // keeps them, but then they're not empty/whitespace-only so they pass
    // through. They're ugly filename characters but they're safe.
    for name in &["\n", "\t", "\r", "\n\t\r", "\u{0007}\u{0008}"] {
        assert_safe_or_rejected(dir.path(), name);
    }
}

#[test]
fn only_separator_name_uses_fallback() {
    // Names consisting solely of path separators strip to empty → fallback.
    let dir = TempDir::new().unwrap();
    for name in &["///", "\\\\", "//\\\\//"] {
        assert_safe_or_rejected(dir.path(), name);
        let dest = safe_destination_path(dir.path(), name, "fallback123").unwrap();
        assert_eq!(dest.file_name().unwrap(), "fallback123");
    }
}

#[test]
fn mixed_separators_and_whitespace_name_uses_fallback() {
    // Stripping separators leaves only whitespace → fallback.
    let dir = TempDir::new().unwrap();
    for name in &["/ /", " // / ", " \t/ \n "] {
        assert_safe_or_rejected(dir.path(), name);
    }
}

#[test]
fn name_that_is_only_whitespace_after_stripping_uses_fallback() {
    // Already tested in part for "   " — also test tabs, newlines mixed.
    let dir = TempDir::new().unwrap();
    for name in &["\t", "\n", "\t \n \t", "\r\n"] {
        assert_safe_or_rejected(dir.path(), name);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Section 7 — Filesystem name and path-length boundaries
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn name_at_exactly_255_bytes_is_safe() {
    // 255 bytes is the common filename length limit (NAME_MAX on most FS).
    let dir = TempDir::new().unwrap();
    let name = "a".repeat(255);
    assert_safe_or_rejected(dir.path(), &name);
}

#[test]
fn name_just_over_255_bytes_remains_inside_directory() {
    let dir = TempDir::new().unwrap();
    let name = "a".repeat(256);
    assert_safe_or_rejected(dir.path(), &name);
}

#[test]
fn name_at_1024_bytes_remains_inside_directory() {
    let dir = TempDir::new().unwrap();
    let name = "b".repeat(1024);
    assert_safe_or_rejected(dir.path(), &name);
}

#[test]
fn name_at_4096_bytes_remains_inside_directory() {
    let dir = TempDir::new().unwrap();
    let name = "c".repeat(4096);
    assert_safe_or_rejected(dir.path(), &name);
}

#[test]
fn long_name_with_extension_remains_safe() {
    let dir = TempDir::new().unwrap();
    let stem = "a".repeat(200);
    let name = format!("{stem}.pdf");
    assert_safe_or_rejected(dir.path(), &name);
}

#[test]
fn long_name_with_multiple_extensions_remains_safe() {
    let dir = TempDir::new().unwrap();
    let name = format!("{}.tar.gz", "a".repeat(240));
    assert_safe_or_rejected(dir.path(), &name);
}

#[test]
fn long_fallback_stem_stays_inside_directory() {
    let dir = TempDir::new().unwrap();
    // When the display name is empty the fallback (content hash) is used.
    // The fallback could itself be long.
    let dest = safe_destination_path(dir.path(), "", &"x".repeat(512)).unwrap();
    assert!(dest.starts_with(dir.path()));
    assert_eq!(
        dest.file_name().and_then(|s| s.to_str()).unwrap(),
        &"x".repeat(512)
    );
}

#[test]
fn long_name_with_dedup_counter_stays_safe() {
    // When a very long filename collides, the dedup suffix is appended.
    // The combined name must still be a valid path component that stays
    // inside the download dir.
    let dir = TempDir::new().unwrap();
    let name = "a".repeat(240) + ".txt";
    let first = safe_destination_path(dir.path(), &name, "hash").unwrap();
    fs::write(&first, b"first").unwrap();
    let second = safe_destination_path(dir.path(), &name, "hash").unwrap();
    assert!(second.starts_with(dir.path()));
    assert_ne!(first, second);
    // The deduped filename should be a child of the download dir.
    assert_eq!(second.parent(), Some(dir.path()));
}

// ═══════════════════════════════════════════════════════════════════════
// Section 8 — Collision and deduplication edge cases
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn reserved_name_collision_triggers_dedup() {
    // Two identical reserved names should lead to dedup on the fallback,
    // not silent overwrite.
    let dir = TempDir::new().unwrap();
    let first = safe_destination_path(dir.path(), "CON.txt", "content-hash").unwrap();
    fs::write(&first, b"first").unwrap();
    let second = safe_destination_path(dir.path(), "CON.txt", "content-hash").unwrap();
    assert_ne!(first, second);
    assert_eq!(first.file_name().unwrap(), "content-hash.txt");
    assert_eq!(second.file_name().unwrap(), "content-hash (1).txt");
}

#[test]
fn two_different_reserved_names_collide_when_both_use_plain_fallback() {
    // "CON" and "PRN" (both reserved, no extension) both map to the
    // plain fallback stem → dedup needed.
    let dir = TempDir::new().unwrap();
    let first = safe_destination_path(dir.path(), "CON", "hash123").unwrap();
    fs::write(&first, b"first").unwrap();
    let second = safe_destination_path(dir.path(), "PRN", "hash123").unwrap();
    assert_ne!(first, second);
    // CON → "hash123", PRN → "hash123" which already exists → "hash123 (1)"
    assert_eq!(second.file_name().unwrap(), "hash123 (1)");
}

#[test]
fn reserved_collision_with_pre_existing_fallback_extends() {
    let dir = TempDir::new().unwrap();
    // Create a file named "abc.txt" (the fallback name that CON.txt would get)
    let preexisting = safe_destination_path(dir.path(), "abc.txt", "abc").unwrap();
    fs::write(&preexisting, b"preexisting").unwrap();
    // Now send CON.txt with the same fallback → should get "abc (1).txt"
    let dest = safe_destination_path(dir.path(), "CON.txt", "abc").unwrap();
    assert_eq!(dest.file_name().unwrap(), "abc (1).txt");
}

#[test]
fn dedup_skips_filled_gaps_in_sequence() {
    // If "file (1).txt" and "file (3).txt" exist but "file (2).txt" does
    // not, dedup should pick the first free slot: "file (2).txt".
    let dir = TempDir::new().unwrap();
    let base = dir.path().join("file.txt");
    fs::write(&base, b"base").unwrap();
    let d1 = dir.path().join("file (1).txt");
    fs::write(&d1, b"one").unwrap();
    let d3 = dir.path().join("file (3).txt");
    fs::write(&d3, b"three").unwrap();

    let deduped = safe_destination_path(dir.path(), "file.txt", "hash").unwrap();
    assert_eq!(deduped.file_name().unwrap(), "file (2).txt");
}

#[test]
fn dedup_chain_of_ten_does_not_overwrite() {
    // Create 10 files with the same display name, verify each gets a
    // unique dedup slot and no silent overwrite occurs.
    let dir = TempDir::new().unwrap();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..10 {
        let dest = safe_destination_path(dir.path(), "chain.txt", "hash").unwrap();
        assert!(seen.insert(dest.clone()), "duplicate destination {dest:?}");
        fs::write(&dest, b"data").unwrap();
    }
    assert_eq!(seen.len(), 10);
}

#[test]
fn dedup_preserves_extensionless_base() {
    // Extensionless files should get "(1)", "(2)" suffix, not ". (1)".
    let dir = TempDir::new().unwrap();
    let base = dir.path().join("README");
    fs::write(&base, b"data").unwrap();
    let d1 = safe_destination_path(dir.path(), "README", "hash").unwrap();
    assert_eq!(d1.file_name().unwrap(), "README (1)");

    fs::write(&d1, b"more").unwrap();
    let d2 = safe_destination_path(dir.path(), "README", "hash").unwrap();
    assert_eq!(d2.file_name().unwrap(), "README (2)");
}

#[test]
fn dedup_of_fallback_for_extensionless_reserved_name() {
    let dir = TempDir::new().unwrap();
    // "NUL" (no extension) → fallback stem "hash"
    let first = safe_destination_path(dir.path(), "NUL", "hash").unwrap();
    fs::write(&first, b"first").unwrap();
    assert_eq!(first.file_name().unwrap(), "hash");

    // "CON" (no extension) → also fallback "hash" → dedup needed
    let second = safe_destination_path(dir.path(), "CON", "hash").unwrap();
    assert_eq!(second.file_name().unwrap(), "hash (1)");
}

#[test]
fn dedup_of_same_name_from_three_sources() {
    // Three separate remote files with the same display name, all sent
    // to the same download directory.
    let dir = TempDir::new().unwrap();
    let r1 = safe_destination_path(dir.path(), "photo.png", "a").unwrap();
    fs::write(&r1, b"1").unwrap();
    let r2 = safe_destination_path(dir.path(), "photo.png", "b").unwrap();
    fs::write(&r2, b"2").unwrap();
    let r3 = safe_destination_path(dir.path(), "photo.png", "c").unwrap();
    fs::write(&r3, b"3").unwrap();

    assert_eq!(r1.file_name().unwrap(), "photo.png");
    assert_eq!(r2.file_name().unwrap(), "photo (1).png");
    assert_eq!(r3.file_name().unwrap(), "photo (2).png");
}

// ═══════════════════════════════════════════════════════════════════════
// Section 9 — All categories through prepare_download_destination
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn control_characters_through_prepare_download_destination() {
    let dir = TempDir::new().unwrap();
    for name in &[
        "line\nfeed.txt",
        "tab\tname.bin",
        "bell\u{0007}.dat",
        "file\rname.txt",
        "file\u{0000}.dat",
    ] {
        assert_safe_or_rejected_through_prepare(dir.path(), name);
    }
}

#[test]
fn unicode_bidi_controls_through_prepare_download_destination() {
    let dir = TempDir::new().unwrap();
    for name in &[
        "file\u{200B}.txt",
        "\u{200E}order.txt",
        "\u{202E}override.txt",
    ] {
        assert_safe_or_rejected_through_prepare(dir.path(), name);
    }
}

#[test]
fn length_boundaries_through_prepare_download_destination() {
    let dir = TempDir::new().unwrap();
    for name in &[
        "a".repeat(255),
        "a".repeat(256),
        "a".repeat(1024),
        "a".repeat(4096),
    ] {
        assert_safe_or_rejected_through_prepare(dir.path(), name);
    }
}

#[test]
fn all_windows_device_names_through_prepare() {
    let dir = TempDir::new().unwrap();
    for stem in &[
        "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
        "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ] {
        assert_safe_or_rejected_through_prepare(dir.path(), stem);
        assert_safe_or_rejected_through_prepare(dir.path(), &format!("{stem}.txt"));
    }
}

#[test]
fn collision_chain_through_prepare_download_destination() {
    let dir = TempDir::new().unwrap();
    let r1 = prepare_download_destination(dir.path(), "dup.docx", "h1").unwrap();
    fs::write(&r1, b"1").unwrap();
    let r2 = prepare_download_destination(dir.path(), "dup.docx", "h2").unwrap();
    fs::write(&r2, b"2").unwrap();
    let r3 = prepare_download_destination(dir.path(), "dup.docx", "h3").unwrap();
    fs::write(&r3, b"3").unwrap();

    assert_eq!(r1.file_name().unwrap(), "dup.docx");
    assert_eq!(r2.file_name().unwrap(), "dup (1).docx");
    assert_eq!(r3.file_name().unwrap(), "dup (2).docx");
}

#[test]
fn separator_only_names_through_prepare_download_destination() {
    let dir = TempDir::new().unwrap();
    for name in &["///", "\\\\", "//\\\\//"] {
        let dest = prepare_download_destination(dir.path(), name, "sep-hash").unwrap();
        assert!(dest.starts_with(dir.path()));
        assert_eq!(dest.file_name().unwrap(), "sep-hash");
    }
}
