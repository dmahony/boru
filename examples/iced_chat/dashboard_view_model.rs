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
    /// Build a progress value from cumulative bytes and an optional total.
    pub(crate) fn from_bytes(bytes: u64, total: Option<u64>) -> Self {
        match total {
            Some(total) if total > 0 => Self::Determinate { bytes, total },
            Some(_) | None if bytes > 0 => Self::Indeterminate { bytes },
            _ => Self::Unknown,
        }
    }

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
    /// FS-16: validated status of a remote-shared descriptor (None for local
    /// items, which have no remote lifecycle).
    pub(crate) remote_status: Option<RemoteItemStatus>,
}

/// FS-16: validated status of a remote-shared descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RemoteItemStatus {
    /// Descriptor is valid and the peer is reachable.
    Available,
    /// Descriptor is valid but the peer is offline — fetchable on return.
    OfflineCached,
    /// The content was already downloaded successfully.
    AlreadyDownloaded,
    /// The share grant expired.
    Expired,
    /// The share was revoked by the owner.
    Revoked,
    /// The descriptor failed validation.
    Invalid,
}

/// FS-16: derive the remote item status from validation + presence + history.
pub(crate) fn remote_item_status(
    has_valid_descriptor: bool,
    peer_online: bool,
    already_downloaded: bool,
    expired: bool,
    revoked: bool,
) -> RemoteItemStatus {
    if !has_valid_descriptor {
        return RemoteItemStatus::Invalid;
    }
    if revoked {
        return RemoteItemStatus::Revoked;
    }
    if expired {
        return RemoteItemStatus::Expired;
    }
    if already_downloaded {
        return RemoteItemStatus::AlreadyDownloaded;
    }
    if peer_online {
        RemoteItemStatus::Available
    } else {
        RemoteItemStatus::OfflineCached
    }
}

/// FS-16: project a validated remote descriptor into a UI row. Returns `None`
/// when the descriptor fails validation (missing required fields), so an
/// untrusted or malformed catalogue entry never renders.
pub(crate) fn project_validated_remote_shared_file(
    owner_id: &str,
    file: &RemoteSharedFile,
    peer_online: bool,
) -> Option<SharedItem> {
    if file.display_name.trim().is_empty() || file.shared_file_id.trim().is_empty() {
        return None;
    }
    Some(SharedItem {
        id: StableId::new(format!("remote:{owner_id}:{}", file.shared_file_id)),
        display_name: file.display_name.clone(),
        mime_type: Some(file.mime_type.clone()),
        size_bytes: Some(file.size_bytes),
        offered: true,
        updated_at_ms: file.updated_at_ms,
        recipients: Vec::new(),
        remote_status: Some(remote_item_status(true, peer_online, false, false, false)),
    })
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
    /// FS-11: outbound transfer state derived from the FS-05 projection.
    pub(crate) state: OutboundState,
    /// FS-11: bounded error summary for failed outbound transfers.
    pub(crate) error: Option<String>,
    /// FS-11: latest attempt number.
    pub(crate) attempt: u32,
}

/// Dashboard-visible outbound transfer state, derived from the projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutboundState {
    Transferring,
    Retrying,
    Verifying,
    Completed,
    Failed,
    Cancelled,
    Disconnected,
}

impl From<boru_core::transfer_state_projection::TransferState> for OutboundState {
    fn from(state: boru_core::transfer_state_projection::TransferState) -> Self {
        use boru_core::transfer_state_projection::TransferState;
        match state {
            TransferState::Active => Self::Transferring,
            TransferState::Verifying => Self::Verifying,
            TransferState::Completed => Self::Completed,
            TransferState::Failed => Self::Failed,
            TransferState::Cancelled => Self::Cancelled,
            TransferState::Disconnected => Self::Disconnected,
        }
    }
}

/// Project an FS-05 outbound transfer record into a compact panel row.
///
/// The peer label is the authenticated peer id string from the projection —
/// the caller resolves it to a verified display identity; it is never read
/// from an untrusted display field. The file label is a UI enrichment keyed
/// by the stable item id (content hash) and falls back to a short hash
/// prefix rather than a fabricated name or local path.
pub(crate) fn outbound_row(
    record: &boru_core::transfer_state_projection::TransferRecord,
    item_labels: &std::collections::HashMap<String, String>,
) -> PeerDownload {
    let peer_id = record
        .peer_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let display_name = item_labels
        .get(&record.item_id)
        .cloned()
        .unwrap_or_else(|| {
            let prefix: String = record.item_id.chars().take(12).collect();
            format!("file {prefix}…")
        });
    let state = OutboundState::from(record.state);
    let state = if state == OutboundState::Transferring && record.attempt > 1 {
        OutboundState::Retrying
    } else {
        state
    };
    PeerDownload {
        id: StableId::new(format!("transfer:{}", record.transfer_id)),
        peer_id: StableId::new(format!("peer:{peer_id}")),
        peer_label: peer_id,
        file_id: StableId::new(format!("item:{}", record.item_id)),
        display_name,
        progress: Progress::from_bytes(record.bytes, record.total_bytes),
        updated_at_ms: record.updated_at_ms,
        state,
        error: record.error.clone(),
        attempt: record.attempt,
    }
}

/// Sort outbound rows newest-first with a stable id tiebreaker.
pub(crate) fn sort_outbound_rows(rows: &mut [PeerDownload]) {
    rows.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
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
        remote_status: None,
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
        remote_status: None,
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

/// Local presence/integrity of a completed download's file on disk.
///
/// The dashboard only claims `Verified` when the recorded destination still
/// exists and its size matches the recorded total; anything less is exposed
/// as a warning or a missing-file state so history never implies a file
/// exists when it does not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocalFileState {
    /// Destination exists and size matches the recorded total.
    Verified,
    /// Destination exists but its size differs from the recorded total
    /// (tampered/truncated/replaced) or it could not be read.
    Warning,
    /// No file at the recorded destination (moved or deleted).
    Missing,
    /// No destination was recorded for this history row.
    Unknown,
}

/// A completed incoming download shown in the Downloaded tab.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CompletedDownloadItem {
    /// Stable history id (download row id).
    pub(crate) id: StableId,
    /// Numeric download row id — used for Open/Reveal/Remove messages.
    pub(crate) row_id: i64,
    /// Content id (content hash) — preserved internally for verification.
    pub(crate) content_id: StableId,
    /// Display filename.
    pub(crate) display_name: String,
    /// MIME type hint.
    pub(crate) mime_type: Option<String>,
    /// Total size in bytes.
    pub(crate) size_bytes: u64,
    /// Source peer display label.
    pub(crate) source_peer: String,
    /// When the download completed (ms since UNIX epoch).
    pub(crate) completed_at_ms: u64,
    /// Local file presence/integrity state.
    pub(crate) local: LocalFileState,
    /// Recorded destination path (used only for safe local actions; never
    /// rendered as raw text).
    pub(crate) destination_path: Option<String>,
}

/// Project a durable completed-download record into a UI-facing row.
///
/// `local` is computed by the caller from the filesystem so this module stays
/// storage/IO-free; the recorded destination path is carried verbatim for
/// Open/Reveal actions but the UI never renders it.
pub(crate) fn project_completed_download(
    record: &boru_core::storage::CompletedDownloadRecord,
    peer_label: &str,
    local: LocalFileState,
) -> CompletedDownloadItem {
    CompletedDownloadItem {
        id: StableId::new(format!("download:{}", record.id)),
        row_id: record.id,
        content_id: StableId::new(format!("content:{}", record.content_hash)),
        display_name: record.display_filename.clone(),
        mime_type: Some(record.mime_type.clone()),
        size_bytes: record.total_bytes,
        source_peer: peer_label.to_string(),
        completed_at_ms: record.completed_at_ms,
        local,
        destination_path: record.destination_path.clone(),
    }
}

/// Order completed downloads newest-first with a stable id tiebreaker.
pub(crate) fn sort_completed_downloads(items: &mut [CompletedDownloadItem]) {
    items.sort_by(|a, b| {
        b.completed_at_ms
            .cmp(&a.completed_at_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
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

    #[test]
    fn completed_download_projection_carries_safe_metadata() {
        let record = boru_core::storage::CompletedDownloadRecord {
            id: 42,
            content_hash: "a".repeat(64),
            remote_peer: "peer-a".into(),
            total_bytes: 1234,
            completed_at_ms: 99,
            destination_path: Some("/home/alice/Downloads/report.pdf".into()),
            display_filename: "report.pdf".into(),
            mime_type: "application/pdf".into(),
        };
        let item = project_completed_download(&record, "Alice", LocalFileState::Verified);
        assert_eq!(item.id.as_str(), "download:42");
        assert_eq!(
            item.content_id.as_str(),
            format!("content:{}", "a".repeat(64))
        );
        assert_eq!(item.display_name, "report.pdf");
        assert_eq!(item.mime_type.as_deref(), Some("application/pdf"));
        assert_eq!(item.size_bytes, 1234);
        assert_eq!(item.source_peer, "Alice");
        assert_eq!(item.completed_at_ms, 99);
        assert_eq!(item.local, LocalFileState::Verified);
        assert_eq!(
            item.destination_path.as_deref(),
            Some("/home/alice/Downloads/report.pdf")
        );
    }

    #[test]
    fn completed_download_projection_keeps_hash_internally_for_verification() {
        let record = boru_core::storage::CompletedDownloadRecord {
            id: 1,
            content_hash: "deadbeef".repeat(8),
            remote_peer: "peer-b".into(),
            total_bytes: 5,
            completed_at_ms: 1,
            destination_path: None,
            display_filename: "x.bin".into(),
            mime_type: "application/octet-stream".into(),
        };
        let item = project_completed_download(&record, "Bob", LocalFileState::Missing);
        assert_eq!(item.local, LocalFileState::Missing);
        assert!(item.destination_path.is_none());
        // The content id preserves the hash for integrity checks without
        // leaking raw protocol noise into the UI.
        assert!(item.content_id.as_str().starts_with("content:"));
    }

    #[test]
    fn completed_downloads_sort_newest_first_with_stable_tiebreak() {
        let record = |id: i64, at: u64| boru_core::storage::CompletedDownloadRecord {
            id,
            content_hash: format!("hash-{id}"),
            remote_peer: "peer".into(),
            total_bytes: 1,
            completed_at_ms: at,
            destination_path: None,
            display_filename: format!("f{id}.bin"),
            mime_type: "application/octet-stream".into(),
        };
        let mut items = vec![
            project_completed_download(&record(1, 10), "p", LocalFileState::Verified),
            project_completed_download(&record(2, 30), "p", LocalFileState::Verified),
            project_completed_download(&record(3, 30), "p", LocalFileState::Verified),
        ];
        sort_completed_downloads(&mut items);
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["download:3", "download:2", "download:1"]);
    }

    #[test]
    fn remote_item_status_precedence_is_invalid_then_revoked_then_expired() {
        // An invalid descriptor wins over everything — an untrusted or
        // malformed offer never renders as available even when every other
        // signal looks fresh.
        assert_eq!(
            remote_item_status(false, true, false, false, false),
            RemoteItemStatus::Invalid
        );
        assert_eq!(
            remote_item_status(false, true, true, true, true),
            RemoteItemStatus::Invalid
        );
        // Revoked beats expired; both beat availability/downloaded state.
        assert_eq!(
            remote_item_status(true, true, false, true, true),
            RemoteItemStatus::Revoked
        );
        assert_eq!(
            remote_item_status(true, true, false, true, false),
            RemoteItemStatus::Expired
        );
        // Already downloaded beats online/offline signals.
        assert_eq!(
            remote_item_status(true, false, true, false, false),
            RemoteItemStatus::AlreadyDownloaded
        );
        // Online → Available; offline → OfflineCached (fetchable on return).
        assert_eq!(
            remote_item_status(true, true, false, false, false),
            RemoteItemStatus::Available
        );
        assert_eq!(
            remote_item_status(true, false, false, false, false),
            RemoteItemStatus::OfflineCached
        );
    }

    #[test]
    fn validated_remote_projection_rejects_malformed_or_untrusted_descriptors() {
        let remote = |name: &str, id: &str| RemoteSharedFile {
            shared_file_id: id.into(),
            display_name: name.into(),
            description: None,
            mime_type: "image/png".into(),
            size_bytes: 99,
            content_hash: "b".repeat(64),
            version_number: 1,
            updated_at_ms: 12,
            collection_ids: vec![],
        };
        // Missing display name → never renders.
        assert!(
            project_validated_remote_shared_file("peer-z", &remote("", "shared-7"), true).is_none()
        );
        // Missing stable shared-file id → never renders.
        assert!(
            project_validated_remote_shared_file("peer-z", &remote("photo.png", ""), true)
                .is_none()
        );
        // Whitespace-only display name is also rejected.
        assert!(
            project_validated_remote_shared_file("peer-z", &remote("   ", "shared-7"), true)
                .is_none()
        );
        // Valid descriptor projects with a stable owner-scoped id.
        let item =
            project_validated_remote_shared_file("peer-z", &remote("photo.png", "shared-7"), true)
                .expect("valid descriptor projects");
        assert_eq!(item.id.as_str(), "remote:peer-z:shared-7");
        assert_eq!(item.remote_status, Some(RemoteItemStatus::Available));
        // Offline peers still project as cached, not invalid or fake-available.
        let item =
            project_validated_remote_shared_file("peer-z", &remote("photo.png", "shared-7"), false)
                .expect("valid descriptor projects offline");
        assert_eq!(item.remote_status, Some(RemoteItemStatus::OfflineCached));
    }

    #[test]
    fn metrics_definitions_count_offered_files_bytes_and_unique_peers() {
        let shared = shared_file();
        let mut offered_a = project_local_shared_file(&shared, None, vec![]);
        offered_a.offered = true;
        let mut offered_b = project_local_shared_file(&shared, None, vec![]);
        offered_b.offered = true;
        let mut not_offered = project_local_shared_file(&shared, None, vec![]);
        not_offered.offered = false;

        let download = |id: i64, bytes: u64, total: u64| Download {
            id,
            content_hash: format!("hash-{id}"),
            remote_peer: "peer-a".into(),
            state: "downloading".into(),
            bytes_downloaded: bytes,
            total_bytes: total,
            created_at_ms: 1,
            updated_at_ms: 2,
            last_error: None,
            retry_count: 0,
            next_retry_at_ms: None,
        };
        let determinate = project_download(&download(1, 10, 100), Some("a.bin"), None);
        let indeterminate = project_download(&download(2, 20, 0), Some("b.bin"), None);
        let unknown = project_download(&download(3, 0, 0), Some("c.bin"), None);

        let mut peer_row = |peer: &str| {
            let record = boru_core::transfer_state_projection::TransferRecord {
                transfer_id: format!("t-{peer}"),
                item_id: "item-1".into(),
                direction: boru_core::transfer_state_projection::TransferDirection::Outbound,
                peer_id: Some(peer.into()),
                bytes: 0,
                total_bytes: None,
                state: boru_core::transfer_state_projection::TransferState::Active,
                started_at_ms: 1,
                updated_at_ms: 2,
                error: None,
                attempt: 1,
            };
            outbound_row(&record, &std::collections::HashMap::new())
        };
        // peer-b appears twice but must count once in the unique-peer metric.
        let peers = vec![peer_row("peer-a"), peer_row("peer-b"), peer_row("peer-b")];

        let metrics = metrics(
            &[offered_a, offered_b, not_offered],
            &peers,
            &[determinate, indeterminate, unknown],
        );
        assert_eq!(metrics.shared_file_count, 2, "only offered items count");
        assert_eq!(
            metrics.transferred_bytes, 30,
            "determinate + indeterminate bytes"
        );
        assert_eq!(metrics.active_peer_count, 2, "unique peers only");
    }
}
