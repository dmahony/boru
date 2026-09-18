//! Runtime capability detection for optional GStreamer inline playback.
//!
//! The supported playback build probes the linked GStreamer registry instead of
//! treating a successful Rust build (or a developer workstation) as proof that
//! a packaged application can decode media. `gst-inspect-1.0` is only recorded
//! as an optional diagnostic aid and is never required during startup.

use std::path::PathBuf;

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
        detect_linked_runtime()
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
    // Do not run gst-inspect here. Apart from being unnecessary for the
    // linked probe below, a diagnostic process can hang while GStreamer is
    // loading plugins and would block application startup.
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(all(feature = "video-playback", not(target_os = "windows")))]
fn detect_linked_runtime() -> VideoRuntimeCapability {
    let inspector = inspector_path();
    if let Err(error) = gstreamer::init() {
        return VideoRuntimeCapability {
            available: false,
            detail: format!(
                "GStreamer core runtime is unavailable ({error}); diagnostic tool: {}.",
                diagnostic_status(&inspector)
            ),
            missing_elements: CORE_ELEMENTS.iter().map(|s| (*s).into()).collect(),
            inspector,
        };
    }

    let missing: Vec<String> = CORE_ELEMENTS
        .iter()
        .filter(|element| gstreamer::ElementFactory::find(element).is_none())
        .map(|element| (*element).to_string())
        .collect();
    let available = missing.is_empty();
    let detail = if available {
        format!(
            "GStreamer core runtime and player elements are available; codec support will be validated when a file is opened. Diagnostic tool: {}.",
            diagnostic_status(&inspector)
        )
    } else {
        format!(
            "GStreamer core runtime is present but required player elements are missing: {}. Diagnostic tool: {}.",
            missing.join(", "),
            diagnostic_status(&inspector)
        )
    };
    VideoRuntimeCapability {
        available,
        detail,
        missing_elements: missing,
        inspector,
    }
}

#[cfg(not(all(feature = "video-playback", not(target_os = "windows"))))]
fn detect_linked_runtime() -> VideoRuntimeCapability {
    VideoRuntimeCapability {
        available: false,
        detail: "Inline video playback is not compiled for this platform or feature set; download and external open remain available.".into(),
        missing_elements: CORE_ELEMENTS.iter().map(|s| (*s).into()).collect(),
        inspector: inspector_path(),
    }
}

#[cfg(any(test, all(feature = "video-playback", not(target_os = "windows"))))]
fn diagnostic_status(inspector: &Option<PathBuf>) -> &'static str {
    if inspector.is_some() {
        "gst-inspect-1.0 found (not required for startup)"
    } else {
        "gst-inspect-1.0 not found (diagnostics unavailable)"
    }
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

    #[test]
    fn diagnostic_tool_status_is_distinct_from_runtime_capability() {
        assert_eq!(
            diagnostic_status(&None),
            "gst-inspect-1.0 not found (diagnostics unavailable)"
        );
        assert_eq!(
            diagnostic_status(&Some(PathBuf::from("/tmp/gst-inspect-1.0"))),
            "gst-inspect-1.0 found (not required for startup)"
        );
    }
}
