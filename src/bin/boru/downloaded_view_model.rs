//! Completed-download tab projection (FS-15).
//!
//! This module translates durable completed-download records
//! (`boru_core::storage::CompletedDownloadRecord`) into stable, owned UI rows
//! for the Downloaded tab. It is deliberately independent of Iced widgets
//! and storage: the application layer feeds it rows plus a best-effort peer
//! name map, and the module owns local-file-state detection, safe action
//! mapping, and ordering.
//!
//! Safety rules:
//! - History removal removes the local dashboard record only; never silently
//!   deletes user files.
//! - If the local item has moved or been deleted, the row retains a clear
//!   "File not found" state.
//! - Open and Reveal in Folder are only offered when the local item still
//!   exists — the view routes them through native OS helpers.
//! - Integrity state is a concise verified/warning/missing taxonomy, never
//!   raw protocol noise.

use boru_core::storage::CompletedDownloadRecord;

/// Upper bound for completed download history retained in the tab.
pub(crate) const MAX_DOWNLOADED_HISTORY: usize = 200;

/// Local-file existence state for a completed download.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocalFileState {
    /// File exists at the recorded path and size matches the expected total.
    Verified,
    /// File exists but size differs — possible truncation or corruption.
    Warning,
    /// File does not exist at the recorded path.
    Missing,
    /// Destination path was never recorded or cannot be checked.
    Unknown,
}

impl LocalFileState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Verified => "Verified",
            Self::Warning => "Integrity warning",
            Self::Missing => "File not found",
            Self::Unknown => "Unknown",
        }
    }

    /// Whether the local file is accessible for open/reveal actions.
    pub(crate) fn exists(self) -> bool {
        matches!(self, Self::Verified | Self::Warning)
    }
}

/// Detect local-file state from a destination path and expected size.
/// This is a pure function — the caller supplies the filesystem check
/// result so the module stays testable and Iced-independent.
pub(crate) fn detect_local_file_state(
    destination: Option<&str>,
    expected_size: u64,
    file_exists: bool,
    actual_size: Option<u64>,
) -> LocalFileState {
    let Some(_dest) = destination else {
        return LocalFileState::Unknown;
    };
    if !file_exists {
        return LocalFileState::Missing;
    }
    match actual_size {
        Some(size) if size == expected_size => LocalFileState::Verified,
        Some(_) => LocalFileState::Warning,
        None => LocalFileState::Verified, // can't check size, assume ok
    }
}

/// A row for the Downloaded tab, derived from one durable completed-download
/// record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DownloadedItem {
    /// Stable download row id.
    pub(crate) id: i64,
    /// Verified content hash.
    pub(crate) content_hash: String,
    /// Display filename from the file object.
    pub(crate) display_name: String,
    /// MIME type hint.
    pub(crate) mime_type: String,
    /// Total expected bytes.
    pub(crate) total_bytes: u64,
    /// Remote peer we downloaded from.
    pub(crate) remote_peer: String,
    /// Resolved peer display name (supplied by the app layer).
    pub(crate) peer_display: String,
    /// When the download completed (ms since UNIX epoch).
    pub(crate) completed_at_ms: u64,
    /// Local file state.
    pub(crate) local: LocalFileState,
    /// Recorded destination path (may be stale).
    pub(crate) destination_path: Option<String>,
}

/// Project a durable completed-download record into a UI row.
///
/// `peer_display` is the resolved display name for the remote peer; the
/// caller resolves it from the authenticated peer id.
/// `local` is the detected local-file state (caller runs the filesystem
/// check via `detect_local_file_state`).
pub(crate) fn project_downloaded_item(
    record: &CompletedDownloadRecord,
    peer_display: String,
    local: LocalFileState,
) -> DownloadedItem {
    DownloadedItem {
        id: record.id,
        content_hash: record.content_hash.clone(),
        display_name: record.display_filename.clone(),
        mime_type: record.mime_type.clone(),
        total_bytes: record.total_bytes,
        remote_peer: record.remote_peer.clone(),
        peer_display,
        completed_at_ms: record.completed_at_ms,
        local,
        destination_path: record.destination_path.clone(),
    }
}

/// Order downloaded items newest-completed-first with a stable id tiebreaker.
pub(crate) fn sort_downloaded_items(items: &mut [DownloadedItem]) {
    items.sort_by(|a, b| {
        b.completed_at_ms
            .cmp(&a.completed_at_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
}

/// Human-readable byte formatting.
pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(id: i64, name: &str, completed_at_ms: u64) -> CompletedDownloadRecord {
        CompletedDownloadRecord {
            id,
            content_hash: format!("hash_{id}"),
            remote_peer: format!("peer_{id}"),
            total_bytes: 1_048_576,
            completed_at_ms,
            destination_path: Some(format!("/downloads/{name}")),
            display_filename: name.to_string(),
            mime_type: "application/octet-stream".to_string(),
        }
    }

    #[test]
    fn project_downloaded_item_preserves_all_fields() {
        let record = make_record(1, "photo.jpg", 5000);
        let item = project_downloaded_item(&record, "Alice".into(), LocalFileState::Verified);
        assert_eq!(item.id, 1);
        assert_eq!(item.display_name, "photo.jpg");
        assert_eq!(item.peer_display, "Alice");
        assert_eq!(item.local, LocalFileState::Verified);
        assert_eq!(item.total_bytes, 1_048_576);
        assert_eq!(item.destination_path, Some("/downloads/photo.jpg".into()));
    }

    #[test]
    fn sort_downloaded_items_newest_first() {
        let r1 = make_record(1, "a.txt", 1000);
        let r2 = make_record(2, "b.txt", 3000);
        let mut items = vec![
            project_downloaded_item(&r1, "P".into(), LocalFileState::Verified),
            project_downloaded_item(&r2, "P".into(), LocalFileState::Verified),
        ];
        sort_downloaded_items(&mut items);
        assert_eq!(items[0].id, 2);
        assert_eq!(items[1].id, 1);
    }

    #[test]
    fn sort_downloaded_items_stable_id_tiebreaker() {
        let r1 = make_record(10, "x.txt", 1000);
        let r2 = make_record(20, "y.txt", 1000);
        let mut items = vec![
            project_downloaded_item(&r1, "P".into(), LocalFileState::Verified),
            project_downloaded_item(&r2, "P".into(), LocalFileState::Verified),
        ];
        sort_downloaded_items(&mut items);
        assert_eq!(items[0].id, 20);
        assert_eq!(items[1].id, 10);
    }

    #[test]
    fn local_file_state_detection() {
        // File exists with matching size
        assert_eq!(
            detect_local_file_state(Some("/tmp/f"), 100, true, Some(100)),
            LocalFileState::Verified
        );
        // File exists with wrong size
        assert_eq!(
            detect_local_file_state(Some("/tmp/f"), 100, true, Some(50)),
            LocalFileState::Warning
        );
        // File missing
        assert_eq!(
            detect_local_file_state(Some("/tmp/f"), 100, false, None),
            LocalFileState::Missing
        );
        // No destination path
        assert_eq!(
            detect_local_file_state(None, 100, true, Some(100)),
            LocalFileState::Unknown
        );
    }

    #[test]
    fn local_file_state_labels() {
        assert_eq!(LocalFileState::Verified.label(), "Verified");
        assert_eq!(LocalFileState::Warning.label(), "Integrity warning");
        assert_eq!(LocalFileState::Missing.label(), "File not found");
        assert_eq!(LocalFileState::Unknown.label(), "Unknown");
    }

    #[test]
    fn exists_only_for_verified_and_warning() {
        assert!(LocalFileState::Verified.exists());
        assert!(LocalFileState::Warning.exists());
        assert!(!LocalFileState::Missing.exists());
        assert!(!LocalFileState::Unknown.exists());
    }
}
