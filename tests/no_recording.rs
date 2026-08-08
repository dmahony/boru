//! Source-level guard for the live call media no-recording boundary.
//!
//! This test intentionally audits the source tree rather than executing a
//! media call. A filesystem write accidentally added to a feature-gated media
//! path could otherwise escape ordinary tests on machines without a camera or
//! audio device.

use std::path::{Path, PathBuf};

const FORBIDDEN_FRAGMENTS: &[&str] = &[
    "use std::fs",
    "std::fs::",
    "use tokio::fs",
    "tokio::fs::",
    "File::create",
    "OpenOptions::new",
    "fs::write",
    "create_dir",
    "create_dir_all",
];

fn rust_sources(root: &Path, output: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(root).expect("call source directory must be readable");
    for entry in entries {
        let entry = entry.expect("call source directory entry must be readable");
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push(path);
        }
    }
}

#[test]
fn call_media_source_has_no_filesystem_write_path() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/call");
    let mut sources = Vec::new();
    rust_sources(&source_root, &mut sources);
    assert!(!sources.is_empty(), "call source audit found no Rust files");

    let mut violations = Vec::new();
    for path in sources {
        let source = std::fs::read_to_string(&path).expect("call source must be valid UTF-8");
        for fragment in FORBIDDEN_FRAGMENTS {
            if source.contains(fragment) {
                violations.push(format!("{} contains `{fragment}`", path.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "live call media must remain memory-only; filesystem API found:\n{}",
        violations.join("\n")
    );
}
