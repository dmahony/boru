//! Runtime capability detection for optional GStreamer inline playback.
//!
//! This module deliberately uses the GStreamer inspection executable instead of
//! treating a successful Rust build (or a developer workstation) as proof that
//! a packaged application can decode media.

use std::path::{Path, PathBuf};
use std::process::Command;

/// GStreamer elements required by the Iced player itself.
pub const CORE_ELEMENTS: &[&str] = &[
    "playbin",
    "decodebin",
    "videoconvert",
    "videoscale",
    "appsink",
];

/// A user-facing snapshot of inline playback support.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoRuntimeCapability {
    /// Whether the core player can be constructed on this machine.
    pub available: bool,
    /// Short reason suitable for a status message or diagnostic log.
    pub detail: String,
    /// Core elements that could not be found, if inspection ran successfully.
    pub missing_elements: Vec<String>,
    /// Executable used for inspection, when one was found.
    pub inspector: Option<PathBuf>,
}

impl VideoRuntimeCapability {
    /// Detect the optional runtime without failing application startup.
    pub fn detect() -> Self {
        let Some(inspector) = inspector_path() else {
            return Self {
                available: false,
                detail: "GStreamer runtime is unavailable (gst-inspect-1.0 was not found). Install Boru's bundled media runtime or the documented system dependency.".into(),
                missing_elements: CORE_ELEMENTS.iter().map(|s| (*s).into()).collect(),
                inspector: None,
            };
        };

        let mut missing = Vec::new();
        for element in CORE_ELEMENTS {
            match inspect(&inspector, element) {
                Ok(true) => {}
                Ok(false) | Err(_) => missing.push((*element).to_string()),
            }
        }
        if missing.is_empty() {
            Self {
                available: true,
                detail: "GStreamer runtime is available; codec support will be validated when a file is opened.".into(),
                missing_elements: missing,
                inspector: Some(inspector),
            }
        } else {
            Self {
                available: false,
                detail: format!(
                    "GStreamer runtime is incomplete; missing core plugin elements: {}.",
                    missing.join(", ")
                ),
                missing_elements: missing,
                inspector: Some(inspector),
            }
        }
    }

    /// Stable fallback text used when inline playback is disabled.
    pub fn unavailable_message(&self) -> String {
        format!(
            "Inline video playback unavailable: {} Download and external open remain available.",
            self.detail
        )
    }
}

fn inspector_path() -> Option<PathBuf> {
    // Packaged Windows builds place the runtime beside the application. The
    // explicit override is also useful for clean-machine tests and distributors.
    if let Some(path) = std::env::var_os("BORU_GST_INSPECT") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    for candidate in [
        exe_dir.join(r"gstreamer\1.0\msvc_x86_64\bin\gst-inspect-1.0.exe"),
        exe_dir.join("gst-inspect-1.0"),
        exe_dir.join("gst-inspect-1.0.exe"),
    ] {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    which("gst-inspect-1.0").or_else(|| which("gst-inspect-1.0.exe"))
}

fn which(name: &str) -> Option<PathBuf> {
    // Use the platform-aware command lookup rather than interpreting PATH
    // ourselves; this handles Windows PATHEXT and Unix executable bits.
    let output = Command::new(name).arg("--version").output().ok()?;
    if output.status.success() {
        Some(PathBuf::from(name))
    } else {
        None
    }
}

fn inspect(inspector: &Path, element: &str) -> std::io::Result<bool> {
    Ok(Command::new(inspector)
        .arg(element)
        .output()?
        .status
        .success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_element_contract_is_explicit() {
        assert!(CORE_ELEMENTS.contains(&"playbin"));
        assert!(CORE_ELEMENTS.contains(&"appsink"));
        assert!(CORE_ELEMENTS.contains(&"videoconvert"));
    }

    #[test]
    fn unavailable_message_preserves_fallback_actions() {
        let capability = VideoRuntimeCapability {
            available: false,
            detail: "missing appsink".into(),
            missing_elements: vec!["appsink".into()],
            inspector: None,
        };
        let message = capability.unavailable_message();
        assert!(message.contains("missing appsink"));
        assert!(message.contains("Download"));
        assert!(message.contains("external open"));
    }
}
