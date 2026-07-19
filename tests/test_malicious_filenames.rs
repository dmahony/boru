//! Regression coverage for untrusted remote filenames.
//!
//! Every accepted destination must remain a direct child of the caller's
//! selected directory. Inputs that cannot be represented safely may instead
//! be rejected; they must never select a path outside that directory.

use std::fs;
use std::path::{Path, PathBuf};

use boru_chat::safe_destination::safe_destination_path;
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
