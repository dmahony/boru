//! Data-only projections for the File Sharing dashboard.
//!
//! This module is intentionally independent of Iced widgets.  It translates
//! authoritative storage/transfer rows into stable, owned values that a view
//! can render without borrowing a database connection, networking object, or
//! local filesystem path.

use boru_core::storage::{Download, FileObject, RemoteSharedFileRow, SharedFileRow};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DashboardTab {
    SharedByMe,
    Downloading,
    Downloaded,
    SharedWithMe,
    ActivityLog,
}

impl DashboardTab {
    pub(crate) const ALL: [Self; 5] = [
        Self::SharedByMe,
        Self::Downloading,
        Self::Downloaded,
        Self::SharedWithMe,
        Self::ActivityLog,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::SharedByMe => "Shared by Me",
            Self::Downloading => "Downloading",
            Self::Downloaded => "Downloaded",
            Self::SharedWithMe => "Shared with Me",
            Self::ActivityLog => "Activity Log",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DashboardLoadState {
    Loading,
    Ready,
    Stale { age_ms: u64 },
    Offline,
    Error(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Progress {
    Indeterminate { bytes: u64 },
    Known { bytes: u64, total: u64 },
}

impl Progress {
    pub(crate) fn from_bytes(bytes: u64, total: Option<u64>) -> Self {
        match total.filter(|total| *total > 0) {
            Some(total) => Self::Known { bytes, total },
            None => Self::Indeterminate { bytes },
        }
    }

    pub(crate) fn percentage(&self) -> Option<u8> {
        match self {
            Self::Indeterminate { .. } | Self::Known { total: 0, .. } => None,
            Self::Known { bytes, total } => Some(
                bytes
                    .saturating_mul(100)
                    .checked_div(*total)
                    .unwrap_or(100)
                    .min(100) as u8,
            ),
        }
    }

    pub(crate) fn bytes(&self) -> u64 {
        match self {
            Self::Indeterminate { bytes } | Self::Known { bytes, .. } => *bytes,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SharedItem {
    pub id: String,
    pub display_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub content_hash: String,
    pub description: Option<String>,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemoteItem {
    pub id: String,
    pub peer_id: String,
    pub display_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub content_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecipientSummary {
    pub peer_id: String,
    pub display_name: String,
    pub shared_item_count: u64,
    pub active_download_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PeerDownload {
    pub id: String,
    pub peer_id: String,
    pub peer_name: String,
    pub file_name: String,
    pub progress: Progress,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TransferRow {
    pub id: String,
    pub file_name: String,
    pub remote_peer: String,
    pub state: String,
    pub progress: Progress,
    pub updated_at_ms: u64,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActivityKind {
    Shared,
    Started,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ActivityEvent {
    pub id: String,
    pub occurred_at_ms: u64,
    pub kind: ActivityKind,
    pub summary: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SharingMetrics {
    pub shared_item_count: u64,
    pub active_peer_count: u64,
    pub bytes_transferred: u64,
    pub completed_download_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DashboardState {
    pub active_tab: DashboardTab,
    pub load_state: DashboardLoadState,
    pub shared_by_me: Vec<SharedItem>,
    pub recipients: Vec<RecipientSummary>,
    pub peers_downloading_from_me: Vec<PeerDownload>,
    pub incoming_downloads: Vec<TransferRow>,
    pub completed_downloads: Vec<TransferRow>,
    pub shared_with_me: Vec<RemoteItem>,
    pub activity: Vec<ActivityEvent>,
    pub metrics: SharingMetrics,
}

impl Default for DashboardState {
    fn default() -> Self {
        Self {
            active_tab: DashboardTab::SharedByMe,
            load_state: DashboardLoadState::Loading,
            shared_by_me: Vec::new(),
            recipients: Vec::new(),
            peers_downloading_from_me: Vec::new(),
            incoming_downloads: Vec::new(),
            completed_downloads: Vec::new(),
            shared_with_me: Vec::new(),
            activity: Vec::new(),
            metrics: SharingMetrics::default(),
        }
    }
}

/// Commands are converted to `AppMessage` by the application layer; views do
/// not mutate storage or start transfers directly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DashboardCommand {
    OpenDownloadsFolder,
    DownloadRemoteItem { item_id: String },
    CancelDownload { transfer_id: String },
    RetryDownload { transfer_id: String },
    RemoveSharedItem { item_id: String },
}

pub(crate) fn local_item_from_domain(row: &SharedFileRow, object: &FileObject) -> SharedItem {
    SharedItem {
        id: format!("shared:{}", row.metadata_id),
        display_name: row.display_filename.clone(),
        mime_type: object.mime_type.clone(),
        size_bytes: object.size,
        content_hash: row.content_hash.clone(),
        description: row.description.clone(),
        updated_at_ms: row.updated_at_ms,
    }
}

pub(crate) fn remote_item_from_domain(peer_id: &str, row: &RemoteSharedFileRow) -> RemoteItem {
    RemoteItem {
        id: format!("remote:{peer_id}:{}", row.content_hash),
        peer_id: peer_id.to_owned(),
        display_name: row.display_filename.clone(),
        mime_type: row.mime_type.clone(),
        size_bytes: row.size_bytes,
        content_hash: row.content_hash.clone(),
    }
}

pub(crate) fn transfer_from_domain(
    download: &Download,
    file_name: impl Into<String>,
) -> TransferRow {
    TransferRow {
        id: format!("download:{}", download.id),
        file_name: file_name.into(),
        remote_peer: download.remote_peer.clone(),
        state: download.state.clone(),
        progress: Progress::from_bytes(download.bytes_downloaded, Some(download.total_bytes)),
        updated_at_ms: download.updated_at_ms,
        error: download.last_error.clone(),
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DownloadProjection {
    pub incoming: Vec<TransferRow>,
    pub completed: Vec<TransferRow>,
}

pub(crate) fn project_downloads(rows: &[Download]) -> DownloadProjection {
    let mut projection = DownloadProjection::default();
    for row in rows {
        let transfer = transfer_from_domain(row, row.content_hash.clone());
        if row.state == "complete" || row.state == "completed" {
            projection.completed.push(transfer);
        } else if !matches!(
            row.state.as_str(),
            "cancelled" | "failed" | "version_mismatch"
        ) {
            projection.incoming.push(transfer);
        }
    }
    projection.incoming.sort_by(|a, b| a.id.cmp(&b.id));
    projection.completed.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
    projection
}

pub(crate) fn sort_activity(events: &mut [ActivityEvent]) {
    events.sort_by(|a, b| {
        b.occurred_at_ms
            .cmp(&a.occurred_at_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use boru_core::storage::{Download, FileObject, RemoteSharedFileRow, SharedFileRow};

    fn local_row(hash: &str, name: &str, updated_at_ms: u64) -> SharedFileRow {
        SharedFileRow {
            content_hash: hash.into(),
            profile_user_id: "local".into(),
            metadata_id: format!("meta-{hash}"),
            display_filename: name.into(),
            description: None,
            offered: true,
            created_at_ms: updated_at_ms,
            updated_at_ms,
            version: 1,
        }
    }

    fn object(hash: &str, name: &str, size: u64) -> FileObject {
        FileObject {
            content_hash: hash.into(),
            size,
            mime_type: "text/plain".into(),
            filename: name.into(),
            created_at_ms: 1,
            data: None,
            source_path: Some("/secret/local/path.txt".into()),
        }
    }

    fn download(id: i64, hash: &str, state: &str, bytes: u64, total: u64) -> Download {
        Download {
            id,
            content_hash: hash.into(),
            remote_peer: "peer-a".into(),
            state: state.into(),
            bytes_downloaded: bytes,
            total_bytes: total,
            created_at_ms: 100,
            updated_at_ms: 200,
            last_error: None,
            retry_count: 0,
            next_retry_at_ms: None,
        }
    }

    #[test]
    fn local_projection_uses_stable_id_and_never_carries_source_path() {
        let item = local_item_from_domain(
            &local_row("hash-b", "B.txt", 20),
            &object("hash-b", "B.txt", 2048),
        );
        assert_eq!(item.id, "shared:meta-hash-b");
        assert_eq!(item.size_bytes, 2048);
        assert!(!format!("{item:?}").contains("/secret/local"));
    }

    #[test]
    fn remote_projection_has_peer_scoped_stable_id_and_no_local_path() {
        let row = RemoteSharedFileRow {
            content_hash: "hash-a".into(),
            display_filename: "A.txt".into(),
            mime_type: "text/plain".into(),
            size_bytes: 12,
        };
        let item = remote_item_from_domain("peer-z", &row);
        assert_eq!(item.id, "remote:peer-z:hash-a");
        assert!(!format!("{item:?}").contains("source_path"));
    }

    #[test]
    fn progress_is_indeterminate_for_unknown_or_zero_totals() {
        assert_eq!(Progress::from_bytes(4, None).percentage(), None);
        assert_eq!(Progress::from_bytes(4, Some(0)).percentage(), None);
        assert_eq!(Progress::from_bytes(25, Some(100)).percentage(), Some(25));
    }

    #[test]
    fn tab_model_contains_all_dashboard_tabs_in_spec_order() {
        assert_eq!(DashboardTab::ALL.len(), 5);
        assert_eq!(DashboardTab::ALL[0].label(), "Shared by Me");
        assert_eq!(DashboardTab::ALL[4].label(), "Activity Log");
    }

    #[test]
    fn downloads_are_projected_into_incoming_and_completed_tabs_deterministically() {
        let rows = vec![
            download(2, "b", "complete", 10, 10),
            download(1, "a", "queued", 0, 0),
            download(3, "c", "downloading", 5, 10),
        ];
        let projection = project_downloads(&rows);
        assert_eq!(
            projection
                .incoming
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["download:1", "download:3"]
        );
        assert_eq!(projection.completed[0].id, "download:2");
        assert_eq!(projection.incoming[0].progress.percentage(), None);
    }

    #[test]
    fn formatting_is_deterministic_and_activity_is_newest_first_with_id_tiebreak() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
        let mut events = vec![
            ActivityEvent {
                id: "b".into(),
                occurred_at_ms: 10,
                kind: ActivityKind::Completed,
                summary: "b".into(),
            },
            ActivityEvent {
                id: "a".into(),
                occurred_at_ms: 10,
                kind: ActivityKind::Shared,
                summary: "a".into(),
            },
        ];
        sort_activity(&mut events);
        assert_eq!(
            events.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["b", "a"]
        );
    }
}
