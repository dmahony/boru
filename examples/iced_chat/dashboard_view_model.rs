//! Presentation projections for the File Sharing dashboard.
//!
//! This module deliberately owns no storage, networking, or widget state. It
//! converts authoritative Boru domain records into stable, UI-facing values.

use boru_core::catalogue_model::RemoteSharedFile;
use boru_core::diagnostics::TransferLifecycleEvent;
use boru_core::storage::{Download, FileObject, SharedFileRow};

/// The five dashboard tabs, in their design-system order.
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

/// Overall freshness/connectivity state for the projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ScreenStatus {
    Loading,
    Ready,
    Stale { observed_at_ms: u64 },
    Offline,
    Error { message: String },
}

/// Stable identifier used by all dashboard rows.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct StableId(String);

impl StableId {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Progress is explicit about missing totals and missing observations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Progress {
    Determinate { bytes: u64, total: u64 },
    Indeterminate { bytes: u64 },
    Unknown,
}

impl Progress {
    pub(crate) fn fraction(&self) -> Option<f32> {
        match self {
            Self::Determinate { bytes, total } if *total > 0 => {
                Some((*bytes as f64 / *total as f64).min(1.0) as f32)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecipientSummary {
    pub(crate) id: StableId,
    pub(crate) label: String,
    pub(crate) access: AccessState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccessState {
    Allowed,
    Denied,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SharedItem {
    pub(crate) id: StableId,
    pub(crate) display_name: String,
    pub(crate) mime_type: Option<String>,
    pub(crate) size_bytes: Option<u64>,
    pub(crate) offered: bool,
    pub(crate) updated_at_ms: u64,
    pub(crate) recipients: Vec<RecipientSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PeerDownload {
    pub(crate) id: StableId,
    pub(crate) peer_id: StableId,
    pub(crate) peer_label: String,
    pub(crate) file_id: StableId,
    pub(crate) display_name: String,
    pub(crate) progress: Progress,
    pub(crate) updated_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DownloadStatus {
    Queued,
    ResolvingPeer,
    RequestingPermission,
    Downloading,
    Verifying,
    Complete,
    Paused,
    Failed,
    Cancelled,
    VersionMismatch,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DownloadRow {
    pub(crate) id: StableId,
    pub(crate) content_id: StableId,
    pub(crate) remote_peer_id: StableId,
    pub(crate) display_name: Option<String>,
    pub(crate) mime_type: Option<String>,
    pub(crate) progress: Progress,
    pub(crate) status: DownloadStatus,
    pub(crate) updated_at_ms: u64,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActivityKind {
    Shared,
    Started,
    Progress,
    Completed,
    Failed,
    Cancelled,
    Notice,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ActivityEvent {
    pub(crate) id: StableId,
    pub(crate) occurred_at_ms: u64,
    pub(crate) kind: ActivityKind,
    pub(crate) label: String,
}

impl ActivityEvent {
    pub(crate) fn new(id: impl Into<String>, occurred_at_ms: u64, kind: ActivityKind) -> Self {
        Self {
            id: StableId::new(id),
            occurred_at_ms,
            label: activity_label(kind).to_string(),
            kind,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SharingMetrics {
    pub(crate) shared_file_count: usize,
    pub(crate) transferred_bytes: u64,
    pub(crate) active_peer_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DashboardState {
    pub(crate) status: ScreenStatus,
    pub(crate) active_tab: DashboardTab,
    pub(crate) shared_by_me: Vec<SharedItem>,
    pub(crate) peers_downloading_from_me: Vec<PeerDownload>,
    pub(crate) downloads: Vec<DownloadRow>,
    pub(crate) shared_with_me: Vec<SharedItem>,
    pub(crate) activity: Vec<ActivityEvent>,
    pub(crate) metrics: SharingMetrics,
}

impl DashboardState {
    pub(crate) fn empty(status: ScreenStatus) -> Self {
        Self {
            status,
            active_tab: DashboardTab::SharedByMe,
            shared_by_me: Vec::new(),
            peers_downloading_from_me: Vec::new(),
            downloads: Vec::new(),
            shared_with_me: Vec::new(),
            activity: Vec::new(),
            metrics: SharingMetrics::default(),
        }
    }
}

pub(crate) fn project_local_shared_file(
    row: &SharedFileRow,
    object: Option<&FileObject>,
    mut recipients: Vec<RecipientSummary>,
) -> SharedItem {
    recipients.sort_by(|a, b| a.id.cmp(&b.id));
    SharedItem {
        id: StableId::new(format!("local:{}:{}", row.profile_user_id, row.metadata_id)),
        display_name: row.display_filename.clone(),
        mime_type: object.map(|value| value.mime_type.clone()),
        size_bytes: object.map(|value| value.size),
        offered: row.offered,
        updated_at_ms: row.updated_at_ms,
        recipients,
    }
}

pub(crate) fn project_remote_shared_file(owner_id: &str, file: &RemoteSharedFile) -> SharedItem {
    SharedItem {
        id: StableId::new(format!("remote:{owner_id}:{}", file.shared_file_id)),
        display_name: file.display_name.clone(),
        mime_type: Some(file.mime_type.clone()),
        size_bytes: Some(file.size_bytes),
        offered: true,
        updated_at_ms: file.updated_at_ms,
        recipients: Vec::new(),
    }
}

pub(crate) fn project_download(
    download: &Download,
    display_name: Option<&str>,
    mime_type: Option<&str>,
) -> DownloadRow {
    let progress = match download.total_bytes {
        total if total > 0 => Progress::Determinate {
            bytes: download.bytes_downloaded,
            total,
        },
        0 if download.bytes_downloaded > 0 => Progress::Indeterminate {
            bytes: download.bytes_downloaded,
        },
        _ => Progress::Unknown,
    };
    DownloadRow {
        id: StableId::new(format!("download:{}", download.id)),
        content_id: StableId::new(format!("content:{}", download.content_hash)),
        remote_peer_id: StableId::new(format!("peer:{}", download.remote_peer)),
        display_name: display_name.map(str::to_owned),
        mime_type: mime_type.map(str::to_owned),
        progress,
        status: download_status(&download.state),
        updated_at_ms: download.updated_at_ms,
        error: download.last_error.clone(),
    }
}

pub(crate) fn project_transfer_event(event: &TransferLifecycleEvent) -> ActivityEvent {
    ActivityEvent {
        id: StableId::new(format!("transfer-event:{}", event.event_id)),
        occurred_at_ms: event.occurred_at_ms,
        kind: activity_kind(&event.event_name),
        label: event.event_name.clone(),
    }
}

pub(crate) fn sort_activity(events: &mut [ActivityEvent]) {
    events.sort_by(|a, b| {
        b.occurred_at_ms
            .cmp(&a.occurred_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
}

pub(crate) fn sort_shared_items(items: &mut [SharedItem]) {
    items.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
}

pub(crate) fn sort_downloads(downloads: &mut [DownloadRow]) {
    downloads.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
}

pub(crate) fn metrics(
    shared_by_me: &[SharedItem],
    peers_downloading_from_me: &[PeerDownload],
    downloads: &[DownloadRow],
) -> SharingMetrics {
    SharingMetrics {
        shared_file_count: shared_by_me.iter().filter(|item| item.offered).count(),
        transferred_bytes: downloads
            .iter()
            .map(|row| match row.progress {
                Progress::Determinate { bytes, .. } | Progress::Indeterminate { bytes } => bytes,
                Progress::Unknown => 0,
            })
            .sum(),
        active_peer_count: peers_downloading_from_me
            .iter()
            .map(|peer| &peer.peer_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
    }
}

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

pub(crate) fn format_progress(progress: &Progress) -> String {
    match progress {
        Progress::Determinate { bytes, total } => {
            format!("{} / {}", format_bytes(*bytes), format_bytes(*total))
        }
        Progress::Indeterminate { bytes } => format!("{} received", format_bytes(*bytes)),
        Progress::Unknown => "Progress unavailable".to_string(),
    }
}

fn download_status(value: &str) -> DownloadStatus {
    match value {
        "queued" => DownloadStatus::Queued,
        "resolving_peer" => DownloadStatus::ResolvingPeer,
        "requesting_permission" => DownloadStatus::RequestingPermission,
        "downloading" => DownloadStatus::Downloading,
        "verifying" => DownloadStatus::Verifying,
        "complete" => DownloadStatus::Complete,
        "paused" => DownloadStatus::Paused,
        "failed" => DownloadStatus::Failed,
        "cancelled" => DownloadStatus::Cancelled,
        "version_mismatch" => DownloadStatus::VersionMismatch,
        _ => DownloadStatus::Unknown,
    }
}

fn activity_kind(value: &str) -> ActivityKind {
    match value {
        "download_queued" | "access_requested" => ActivityKind::Started,
        "transfer_started" => ActivityKind::Started,
        "progress_checkpoint" => ActivityKind::Progress,
        "completion" => ActivityKind::Completed,
        "failure" => ActivityKind::Failed,
        "cancellation" => ActivityKind::Cancelled,
        _ => ActivityKind::Notice,
    }
}

fn activity_label(kind: ActivityKind) -> &'static str {
    match kind {
        ActivityKind::Shared => "Shared",
        ActivityKind::Started => "Started",
        ActivityKind::Progress => "Progress",
        ActivityKind::Completed => "Completed",
        ActivityKind::Failed => "Failed",
        ActivityKind::Cancelled => "Cancelled",
        ActivityKind::Notice => "Notice",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boru_core::catalogue_model::RemoteSharedFile;
    use boru_core::storage::{Download, FileObject, SharedFileRow};

    fn shared_file() -> SharedFileRow {
        SharedFileRow {
            content_hash: "a".repeat(64),
            profile_user_id: "local-peer".into(),
            metadata_id: "metadata-1".into(),
            display_filename: "report.pdf".into(),
            description: None,
            offered: true,
            created_at_ms: 10,
            updated_at_ms: 20,
            version: 3,
        }
    }

    #[test]
    fn tabs_have_design_order_and_stable_labels() {
        assert_eq!(
            DashboardTab::ALL,
            [
                DashboardTab::SharedByMe,
                DashboardTab::Downloading,
                DashboardTab::Downloaded,
                DashboardTab::SharedWithMe,
                DashboardTab::ActivityLog
            ]
        );
        assert_eq!(DashboardTab::SharedWithMe.label(), "Shared with Me");
    }

    #[test]
    fn local_projection_uses_metadata_id_and_never_copies_source_path() {
        let object = FileObject {
            content_hash: "a".repeat(64),
            size: 42,
            mime_type: "application/pdf".into(),
            filename: "report.pdf".into(),
            created_at_ms: 10,
            data: None,
            source_path: Some("/home/alice/private/report.pdf".into()),
        };
        let item = project_local_shared_file(&shared_file(), Some(&object), vec![]);
        assert_eq!(item.id.as_str(), "local:local-peer:metadata-1");
        assert_eq!(item.size_bytes, Some(42));
        assert!(!format!("{item:?}").contains("/home/alice"));
    }

    #[test]
    fn remote_projection_has_stable_owner_scoped_id() {
        let remote = RemoteSharedFile {
            shared_file_id: "shared-7".into(),
            display_name: "photo.png".into(),
            description: None,
            mime_type: "image/png".into(),
            size_bytes: 99,
            content_hash: "b".repeat(64),
            version_number: 1,
            updated_at_ms: 12,
            collection_ids: vec![],
        };
        assert_eq!(
            project_remote_shared_file("peer-z", &remote).id.as_str(),
            "remote:peer-z:shared-7"
        );
    }

    #[test]
    fn missing_local_object_keeps_projection_safe_and_incomplete() {
        let item = project_local_shared_file(&shared_file(), None, vec![]);
        assert_eq!(item.size_bytes, None);
        assert_eq!(item.mime_type, None);
        assert!(item.offered);
    }

    #[test]
    fn zero_download_total_is_indeterminate_and_never_a_fake_percentage() {
        let download = Download {
            id: 7,
            content_hash: "c".repeat(64),
            remote_peer: "peer-a".into(),
            state: "downloading".into(),
            bytes_downloaded: 10,
            total_bytes: 0,
            created_at_ms: 1,
            updated_at_ms: 2,
            last_error: None,
            retry_count: 0,
            next_retry_at_ms: None,
        };
        let row = project_download(&download, Some("data.bin"), None);
        assert_eq!(row.progress, Progress::Indeterminate { bytes: 10 });
        assert_eq!(row.progress.fraction(), None);
    }

    #[test]
    fn ordering_is_deterministic_with_timestamp_and_id_tiebreakers() {
        let mut events = vec![
            ActivityEvent::new("b", 100, ActivityKind::Completed),
            ActivityEvent::new("a", 100, ActivityKind::Started),
            ActivityEvent::new("c", 99, ActivityKind::Failed),
        ];
        sort_activity(&mut events);
        assert_eq!(
            events.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn formatting_is_byte_based_and_missing_data_is_explicit() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1_048_576), "1.0 MiB");
        assert_eq!(
            format_progress(&Progress::Indeterminate { bytes: 12 }),
            "12 B received"
        );
        assert_eq!(format_progress(&Progress::Unknown), "Progress unavailable");
    }

    #[test]
    fn empty_state_is_ready_without_fake_data() {
        let state = DashboardState::empty(ScreenStatus::Loading);
        assert_eq!(state.active_tab, DashboardTab::SharedByMe);
        assert!(state.shared_by_me.is_empty());
        assert_eq!(DashboardTab::ALL.len(), 5);
    }
}
