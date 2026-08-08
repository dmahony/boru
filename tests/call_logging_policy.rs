//! Source-level guard for the live call logging boundary.
//!
//! The audio/video/media modules are optional and device-dependent, so a source
//! audit is more reliable than an integration test that needs a camera or
//! microphone. Lifecycle logs, if added later, belong in the call-control actor
//! (`manager.rs`) and must use only the fields documented in `LOGGING.md`.

use std::path::{Path, PathBuf};

const HOT_MEDIA_ROOTS: &[&str] = &["audio", "video"];
const HOT_MEDIA_FILES: &[&str] = &["media.rs"];
const FORBIDDEN_LOG_MACROS: &[&str] = &[
    "trace!",
    "debug!",
    "info!",
    "warn!",
    "error!",
    "log::trace!",
    "log::debug!",
    "log::info!",
    "log::warn!",
    "log::error!",
    "println!",
    "eprintln!",
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

fn is_hot_media_path(path: &Path) -> bool {
    let relative = path
        .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/call"))
        .expect("audited file must be below src/call");
    let components: Vec<_> = relative.components().collect();
    relative
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| HOT_MEDIA_FILES.contains(&name))
        || components
            .first()
            .and_then(|part| part.as_os_str().to_str())
            .is_some_and(|name| HOT_MEDIA_ROOTS.contains(&name))
}

#[test]
fn media_hot_paths_have_no_log_statements() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/call");
    let mut sources = Vec::new();
    rust_sources(&source_root, &mut sources);
    assert!(!sources.is_empty(), "call source audit found no Rust files");

    let mut violations = Vec::new();
    for path in sources.into_iter().filter(|path| is_hot_media_path(path)) {
        let source = std::fs::read_to_string(&path).expect("call source must be valid UTF-8");
        for fragment in FORBIDDEN_LOG_MACROS {
            if source.contains(fragment) {
                violations.push(format!("{} contains `{fragment}`", path.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "live media hot paths must not emit logs:\n{}",
        violations.join("\n")
    );
}
