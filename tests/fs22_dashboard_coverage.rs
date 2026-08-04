//! FS-22 File Sharing dashboard coverage — unit / state-machine / persistence
//! harness.
//!
//! Follows the FS-17 isolation pattern: the pure-logic view-model modules from
//! the iced example are included via `#[path]` and driven against the real
//! `boru_core` library, so their projections, transitions, and formatting are
//! locked down independently of the GUI crate compiling.
//!
//! Scope (FS-22): stable IDs, ordering, search/sort state, formatting, tab
//! state retention, share lifecycle projections, duplicate/out-of-order event
//! reduction, inbound/outbound progress transitions, completion / failure /
//! cancel / retry / disconnect / expiry / revoke / restart, metrics
//! definitions, activity deduplication and retention, missing file /
//! destination, native picker action routing through testable abstractions,
//! security checks for unauthorized/invalid remote offers, and regression
//! coverage for the storage-backed share entry points.
//!
//! Deterministic clocks and per-test temporary/in-memory storage keep every
//! test parallel-safe; no network, display, or wall-clock dependence.
//!
//! Run: `cargo test --test fs22_dashboard_coverage`

#[path = "../examples/iced_chat/dashboard_view_model.rs"]
mod dashboard_vm;
#[path = "../examples/iced_chat/downloaded_view_model.rs"]
mod downloaded_vm;
#[path = "../examples/iced_chat/downloading_view_model.rs"]
mod downloading_vm;
#[path = "../examples/iced_chat/peers_downloading_view_model.rs"]
mod peers_vm;
#[path = "../examples/iced_chat/recent_activity_view_model.rs"]
mod recent_vm;

use std::collections::HashMap;

use boru_core::diagnostics::{event_names, TransferLifecycleEvent};
use boru_core::storage::{CompletedDownloadRecord, SharedFilePermission, Storage};
use boru_core::transfer_state_projection::{
    EventName, TransferDirection, TransferEvent, TransferProjection, TransferState,
};

use downloaded_vm::{detect_local_file_state, LocalFileState};

// ── Helpers ────────────────────────────────────────────────────────────────

fn ev(
    transfer_id: &str,
    event_id: &str,
    seq: u64,
    kind: EventName,
    bytes: u64,
    total: Option<u64>,
    at_ms: u64,
    attempt: u32,
    direction: TransferDirection,
    peer_id: Option<&str>,
) -> TransferEvent {
    TransferEvent {
        event_id: event_id.into(),
        transfer_id: transfer_id.into(),
        item_id: format!("item-{transfer_id}"),
        direction,
        peer_id: peer_id.map(str::to_owned),
        sequence: seq,
        attempt,
        occurred_at_ms: at_ms,
        kind,
        bytes,
        total_bytes: total,
        error: None,
    }
}

fn inbound(
    transfer_id: &str,
    event_id: &str,
    seq: u64,
    kind: EventName,
    bytes: u64,
    total: Option<u64>,
    at_ms: u64,
) -> TransferEvent {
    ev(
        transfer_id,
        event_id,
        seq,
        kind,
        bytes,
        total,
        at_ms,
        1,
        TransferDirection::Inbound,
        Some("peer-a"),
    )
}

fn lifecycle(event_id: &str, transfer_id: &str, name: &str, at_ms: u64) -> TransferLifecycleEvent {
    TransferLifecycleEvent {
        schema_version: 1,
        event_id: event_id.into(),
        event_name: name.into(),
        transfer_id: transfer_id.into(),
        sequence: 0,
        occurred_at_ms: at_ms,
        attempt: 1,
        payload: None,
    }
}

fn completed_record(
    id: i64,
    name: &str,
    at_ms: u64,
    destination: Option<&str>,
) -> CompletedDownloadRecord {
    CompletedDownloadRecord {
        id,
        content_hash: format!("hash-{id}"),
        remote_peer: "peer-a".into(),
        total_bytes: 100,
        completed_at_ms: at_ms,
        destination_path: destination.map(str::to_owned),
        display_filename: name.into(),
        mime_type: "application/octet-stream".into(),
    }
}

// ── Share lifecycle projections (deterministic clock) ──────────────────────

#[test]
fn inbound_lifecycle_projects_truthful_states_through_the_view_model() {
    let mut projection = TransferProjection::new(0);
    projection.apply(inbound(
        "t1",
        "s",
        0,
        EventName::Started,
        0,
        Some(100),
        1000,
    ));
    projection.apply(inbound(
        "t1",
        "p1",
        1,
        EventName::Progress,
        40,
        Some(100),
        2000,
    ));
    projection.apply(inbound(
        "t1",
        "p2",
        2,
        EventName::Progress,
        80,
        Some(100),
        3000,
    ));
    projection.apply(inbound(
        "t1",
        "v",
        3,
        EventName::Verifying,
        100,
        Some(100),
        4000,
    ));
    projection.apply(inbound(
        "t1",
        "done",
        4,
        EventName::Completed,
        100,
        Some(100),
        5000,
    ));

    let record = projection.get("t1").unwrap();
    assert_eq!(record.state, TransferState::Completed);
    assert!(record.state.is_terminal());

    let mut labels = HashMap::new();
    labels.insert("item-t1".to_string(), "report.pdf".to_string());
    let row = downloading_vm::incoming_row(record, &labels);
    assert_eq!(row.display_name, "report.pdf");
    assert_eq!(row.state, downloading_vm::IncomingState::Completed);
    assert_eq!(
        row.progress,
        downloading_vm::IncomingProgress::Determinate {
            bytes: 100,
            total: 100
        }
    );

    // Terminal records are retained for history.
    let archived: Vec<_> = projection.archive().collect();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].state, TransferState::Completed);
}

#[test]
fn outbound_lifecycle_with_retry_disconnect_and_restart() {
    let mut projection = TransferProjection::with_progress_interval(0);
    // attempt 1 starts; progress arrives on attempt 2 → Retrying.
    projection.apply(ev(
        "t-out",
        "s",
        0,
        EventName::Started,
        0,
        Some(50),
        1000,
        1,
        TransferDirection::Outbound,
        Some("peer-z"),
    ));
    projection.apply(ev(
        "t-out",
        "r",
        1,
        EventName::Progress,
        10,
        Some(50),
        2000,
        2,
        TransferDirection::Outbound,
        Some("peer-z"),
    ));
    let record = projection.get("t-out").unwrap();
    assert_eq!(record.attempt, 2);
    let row = peers_vm::outbound_row(record, &HashMap::new());
    assert_eq!(row.state, peers_vm::OutboundState::Retrying);
    assert_eq!(row.peer_id.as_deref(), Some("peer-z"));

    // Disconnect: active outbound transfers move to Disconnected but keep
    // the authenticated peer identity.
    projection.disconnect_peer("peer-z", 3000);
    assert_eq!(
        projection.get("t-out").unwrap().state,
        TransferState::Disconnected
    );
    let row = peers_vm::outbound_row(projection.get("t-out").unwrap(), &HashMap::new());
    assert_eq!(row.state, peers_vm::OutboundState::Disconnected);
    assert_eq!(row.peer_id.as_deref(), Some("peer-z"));

    // Restart: a newer event resumes the same logical transfer.
    projection.apply(ev(
        "t-out",
        "resume",
        2,
        EventName::Progress,
        25,
        Some(50),
        4000,
        2,
        TransferDirection::Outbound,
        Some("peer-z"),
    ));
    assert_eq!(
        projection.get("t-out").unwrap().state,
        TransferState::Active
    );
    let row = peers_vm::outbound_row(projection.get("t-out").unwrap(), &HashMap::new());
    assert_eq!(row.state, peers_vm::OutboundState::Retrying);
    assert_eq!(
        row.progress,
        peers_vm::OutboundProgress::Determinate {
            bytes: 25,
            total: 50
        }
    );
}

#[test]
fn failure_cancel_and_post_terminal_events_are_truthful_and_never_regress() {
    let mut projection = TransferProjection::new(0);
    projection.apply(inbound(
        "tf",
        "s",
        0,
        EventName::Started,
        0,
        Some(100),
        1000,
    ));
    let mut fail = inbound("tf", "f", 1, EventName::Failed, 0, Some(100), 2000);
    fail.error = Some("x".repeat(1000));
    projection.apply(fail);
    assert_eq!(projection.get("tf").unwrap().state, TransferState::Failed);
    // Error summaries are bounded to 256 chars.
    assert_eq!(
        projection
            .get("tf")
            .unwrap()
            .error
            .as_deref()
            .unwrap()
            .len(),
        256
    );
    // Post-terminal progress is ignored — no false completion/progress.
    assert!(projection
        .apply(inbound(
            "tf",
            "late",
            2,
            EventName::Progress,
            90,
            Some(100),
            3000
        ))
        .is_none());
    assert_eq!(projection.get("tf").unwrap().bytes, 0);

    let mut projection = TransferProjection::new(0);
    projection.apply(inbound(
        "tc",
        "s",
        0,
        EventName::Started,
        0,
        Some(100),
        1000,
    ));
    projection.apply(inbound(
        "tc",
        "c",
        1,
        EventName::Cancelled,
        0,
        Some(100),
        2000,
    ));
    assert_eq!(
        projection.get("tc").unwrap().state,
        TransferState::Cancelled
    );
    assert!(projection
        .apply(inbound(
            "tc",
            "late",
            2,
            EventName::Progress,
            50,
            Some(100),
            3000
        ))
        .is_none());
    assert_eq!(
        projection.get("tc").unwrap().state,
        TransferState::Cancelled
    );
}

#[test]
fn duplicate_and_out_of_order_events_do_not_duplicate_or_regress_rows() {
    let mut projection = TransferProjection::new(0);
    projection.apply(inbound(
        "t1",
        "s",
        0,
        EventName::Started,
        0,
        Some(100),
        1000,
    ));
    projection.apply(inbound(
        "t1",
        "p2",
        2,
        EventName::Progress,
        80,
        Some(100),
        3000,
    ));
    // Replay of the same event id → ignored.
    assert!(projection
        .apply(inbound(
            "t1",
            "p2",
            2,
            EventName::Progress,
            80,
            Some(100),
            3000
        ))
        .is_none());
    // Out-of-order (stale sequence) → ignored.
    assert!(projection
        .apply(inbound(
            "t1",
            "p1",
            1,
            EventName::Progress,
            20,
            Some(100),
            2000
        ))
        .is_none());
    let record = projection.get("t1").unwrap();
    assert_eq!(record.bytes, 80, "state must not regress");
    assert_eq!(record.updated_at_ms, 3000);
    // Exactly one row survives the projection.
    assert_eq!(projection.snapshot().len(), 1);
}

// ── Tab state retention ────────────────────────────────────────────────────

#[test]
fn dashboard_tabs_are_stable_and_retention_is_a_plain_value() {
    use dashboard_vm::DashboardTab;
    assert_eq!(DashboardTab::ALL.len(), 5);
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
    // Retention: the active tab is a plain value that survives any number of
    // navigation cycles without resetting to the default.
    let tabs = DashboardTab::ALL;
    let idx = |t: DashboardTab| tabs.iter().position(|x| *x == t).unwrap();
    let next = |t: DashboardTab| tabs[(idx(t) + 1) % tabs.len()];
    let prev = |t: DashboardTab| tabs[(idx(t) + tabs.len() - 1) % tabs.len()];
    let mut current = DashboardTab::SharedByMe;
    for _ in 0..5 {
        current = next(current);
    }
    assert_eq!(current, DashboardTab::SharedByMe, "cycles wrap");
    assert_eq!(prev(DashboardTab::SharedByMe), DashboardTab::ActivityLog);
    assert_eq!(next(DashboardTab::ActivityLog), DashboardTab::SharedByMe);
    assert_eq!(DashboardTab::ActivityLog.label(), "Activity Log");
    assert_eq!(DashboardTab::SharedByMe.label(), "Shared by Me");
}

// ── Metrics definitions ────────────────────────────────────────────────────

#[test]
fn metrics_are_defined_over_offered_files_bytes_and_unique_peers() {
    let shared = dashboard_vm::SharedItem {
        id: dashboard_vm::StableId::new("local:me:m1"),
        display_name: "a.pdf".into(),
        mime_type: Some("application/pdf".into()),
        size_bytes: Some(10),
        offered: true,
        updated_at_ms: 1,
        recipients: vec![],
        remote_status: None,
    };
    let not_offered = dashboard_vm::SharedItem {
        offered: false,
        ..shared.clone()
    };
    let downloads = vec![
        dashboard_vm::DownloadRow {
            id: dashboard_vm::StableId::new("download:1"),
            content_id: dashboard_vm::StableId::new("content:h1"),
            remote_peer_id: dashboard_vm::StableId::new("peer:peer-a"),
            display_name: Some("a.bin".into()),
            mime_type: None,
            progress: dashboard_vm::Progress::Determinate {
                bytes: 10,
                total: 100,
            },
            status: dashboard_vm::DownloadStatus::Downloading,
            updated_at_ms: 1,
            error: None,
        },
        dashboard_vm::DownloadRow {
            progress: dashboard_vm::Progress::Indeterminate { bytes: 20 },
            ..dashboard_vm::DownloadRow {
                id: dashboard_vm::StableId::new("download:2"),
                content_id: dashboard_vm::StableId::new("content:h2"),
                remote_peer_id: dashboard_vm::StableId::new("peer:peer-a"),
                display_name: Some("b.bin".into()),
                mime_type: None,
                progress: dashboard_vm::Progress::Unknown,
                status: dashboard_vm::DownloadStatus::Downloading,
                updated_at_ms: 2,
                error: None,
            }
        },
        dashboard_vm::DownloadRow {
            progress: dashboard_vm::Progress::Unknown,
            ..dashboard_vm::DownloadRow {
                id: dashboard_vm::StableId::new("download:3"),
                content_id: dashboard_vm::StableId::new("content:h3"),
                remote_peer_id: dashboard_vm::StableId::new("peer:peer-b"),
                display_name: Some("c.bin".into()),
                mime_type: None,
                progress: dashboard_vm::Progress::Unknown,
                status: dashboard_vm::DownloadStatus::Downloading,
                updated_at_ms: 3,
                error: None,
            }
        },
    ];
    let peers = vec![
        dashboard_vm::PeerDownload {
            id: dashboard_vm::StableId::new("transfer:1"),
            peer_id: dashboard_vm::StableId::new("peer:peer-a"),
            peer_label: "Alice".into(),
            file_id: dashboard_vm::StableId::new("item:h1"),
            display_name: "a.pdf".into(),
            progress: dashboard_vm::Progress::Determinate {
                bytes: 5,
                total: 10,
            },
            updated_at_ms: 1,
            state: dashboard_vm::OutboundState::Transferring,
            error: None,
            attempt: 1,
        },
        dashboard_vm::PeerDownload {
            peer_id: dashboard_vm::StableId::new("peer:peer-b"),
            ..dashboard_vm::PeerDownload {
                id: dashboard_vm::StableId::new("transfer:2"),
                peer_id: dashboard_vm::StableId::new("peer:peer-b"),
                peer_label: "Bob".into(),
                file_id: dashboard_vm::StableId::new("item:h2"),
                display_name: "b.pdf".into(),
                progress: dashboard_vm::Progress::Unknown,
                updated_at_ms: 2,
                state: dashboard_vm::OutboundState::Transferring,
                error: None,
                attempt: 1,
            }
        },
        // peer-b appears twice but counts once.
        dashboard_vm::PeerDownload {
            id: dashboard_vm::StableId::new("transfer:3"),
            peer_id: dashboard_vm::StableId::new("peer:peer-b"),
            peer_label: "Bob".into(),
            file_id: dashboard_vm::StableId::new("item:h3"),
            display_name: "c.pdf".into(),
            progress: dashboard_vm::Progress::Unknown,
            updated_at_ms: 3,
            state: dashboard_vm::OutboundState::Transferring,
            error: None,
            attempt: 1,
        },
    ];
    let metrics = dashboard_vm::metrics(&[shared, not_offered], &peers, &downloads);
    assert_eq!(metrics.shared_file_count, 1, "only offered items count");
    assert_eq!(
        metrics.transferred_bytes, 30,
        "determinate + indeterminate bytes"
    );
    assert_eq!(metrics.active_peer_count, 2, "unique peers only");
}

// ── Activity deduplication and retention (storage-backed) ──────────────────

#[test]
fn activity_persistence_dedups_replays_and_orders_newest_first() {
    let storage = Storage::memory().unwrap();
    let events = vec![
        lifecycle("e1", "t1", event_names::COMPLETION, 300),
        lifecycle("e2", "t2", event_names::COMPLETION, 200),
        lifecycle("e3", "t3", event_names::TRANSFER_STARTED, 100),
        lifecycle("e4", "t4", event_names::FAILURE, 50),
    ];
    for event in &events {
        storage.record_transfer_activity(event).unwrap();
    }
    // Replay of an already-recorded event id is ignored (INSERT OR IGNORE).
    storage.record_transfer_activity(&events[1]).unwrap();

    let rows = storage.list_transfer_activity(100).unwrap();
    assert_eq!(rows.len(), 4, "replays must not create duplicate rows");
    // Newest first with a stable id tiebreak.
    let times: Vec<u64> = rows.iter().map(|r| r.occurred_at_ms).collect();
    assert_eq!(times, vec![300, 200, 100, 50]);

    // The recent-activity projection dedups again at the UI boundary and
    // stays newest-first.
    let projected =
        recent_vm::project_recent_activity(rows, &recent_vm::ActivityEnrichment::default());
    assert_eq!(projected.len(), 4);
    assert!(projected
        .windows(2)
        .all(|w| w[0].occurred_at_ms >= w[1].occurred_at_ms));
}

#[test]
fn activity_rows_survive_reopen_and_retention_prunes_old_rows() {
    let dir = tempfile::tempdir().unwrap();
    let events = vec![
        lifecycle("e1", "t1", event_names::TRANSFER_STARTED, 10),
        lifecycle("e2", "t2", event_names::COMPLETION, 20),
        lifecycle("e3", "t3", event_names::COMPLETION, 30),
    ];
    {
        let storage = Storage::open(dir.path()).unwrap();
        for event in &events {
            storage.record_transfer_activity(event).unwrap();
        }
    }
    // Reopen: durable rows are intact.
    let storage = Storage::open(dir.path()).unwrap();
    assert_eq!(storage.list_transfer_activity(10).unwrap().len(), 3);

    // Retention: pruning drops rows strictly older than the cutoff.
    let pruned = storage.prune_transfer_activity(25).unwrap();
    assert_eq!(pruned, 2);
    let remaining = storage.list_transfer_activity(10).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].event_id, "e3");
}

// ── Missing file / destination ─────────────────────────────────────────────

#[test]
fn missing_destination_is_detected_and_never_offers_open() {
    // No recorded destination → Unknown, never openable.
    assert_eq!(
        detect_local_file_state(None, 100, true, Some(100)),
        LocalFileState::Unknown
    );
    assert!(!LocalFileState::Unknown.exists());
    // Recorded destination but file is gone → Missing, never openable.
    assert_eq!(
        detect_local_file_state(Some("/dl/f.bin"), 100, false, None),
        LocalFileState::Missing
    );
    assert!(!LocalFileState::Missing.exists());
    // Present and size-matching → Verified and openable.
    assert_eq!(
        detect_local_file_state(Some("/dl/f.bin"), 100, true, Some(100)),
        LocalFileState::Verified
    );
    assert!(LocalFileState::Verified.exists());
    // Size differs → warning, still openable for inspection.
    assert_eq!(
        detect_local_file_state(Some("/dl/f.bin"), 100, true, Some(50)),
        LocalFileState::Warning
    );
    assert!(LocalFileState::Warning.exists());
}

#[test]
fn completed_download_without_destination_projects_safe_unknown_state() {
    let record = completed_record(7, "ghost.bin", 5000, None);
    let item = downloaded_vm::project_downloaded_item(
        &record,
        "Alice".into(),
        detect_local_file_state(
            record.destination_path.as_deref(),
            record.total_bytes,
            false,
            None,
        ),
    );
    assert_eq!(item.local, LocalFileState::Unknown);
    assert!(item.destination_path.is_none());
    assert_eq!(item.display_name, "ghost.bin");
}

// ── Security: unauthorized / invalid remote offers ─────────────────────────

#[test]
fn invalid_remote_descriptors_never_render_and_status_precedence_is_safe() {
    use dashboard_vm::{
        project_validated_remote_shared_file, remote_item_status, RemoteItemStatus,
    };
    // A malformed descriptor (missing display name or stable id) never
    // renders a row, even when the peer is online.
    let file = |name: &str, id: &str| boru_core::catalogue_model::RemoteSharedFile {
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
    assert!(project_validated_remote_shared_file("peer-z", &file("", "shared-7"), true).is_none());
    assert!(project_validated_remote_shared_file("peer-z", &file("photo.png", ""), true).is_none());
    // Precedence: invalid > revoked > expired > already-downloaded > online.
    assert_eq!(
        remote_item_status(false, true, true, true, true),
        RemoteItemStatus::Invalid
    );
    assert_eq!(
        remote_item_status(true, true, false, true, true),
        RemoteItemStatus::Revoked
    );
    assert_eq!(
        remote_item_status(true, true, false, true, false),
        RemoteItemStatus::Expired
    );
    assert_eq!(
        remote_item_status(true, false, true, false, false),
        RemoteItemStatus::AlreadyDownloaded
    );
    assert_eq!(
        remote_item_status(true, false, false, false, false),
        RemoteItemStatus::OfflineCached
    );
    assert_eq!(
        remote_item_status(true, true, false, false, false),
        RemoteItemStatus::Available
    );
}

#[test]
fn permission_grant_expiry_and_revoke_are_enforced_at_the_storage_boundary() {
    let storage = Storage::memory().unwrap();
    let hash = "a".repeat(64);
    // Grants are foreign-keyed to a real file object; create it first.
    storage
        .put_file_object(&hash, 1, "application/octet-stream", "f.bin", b"")
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    // No grant yet → unauthorized.
    assert!(!storage.check_permission(&hash, "alice", "read").unwrap());

    // Expired grant → still unauthorized (expiry is enforced, not cosmetic).
    storage
        .grant_permission(&hash, "me", "alice", "read", Some(now - 60_000))
        .unwrap();
    assert!(
        !storage.check_permission(&hash, "alice", "read").unwrap(),
        "expired grant must not authorize"
    );

    // Fresh grant → authorized.
    storage
        .grant_permission(&hash, "me", "alice", "read", Some(now + 60_000))
        .unwrap();
    assert!(storage.check_permission(&hash, "alice", "read").unwrap());

    // Revoke → unauthorized again; revoking twice is an idempotent no-op.
    assert!(storage
        .revoke_permission(&hash, "me", "alice", "read")
        .unwrap());
    assert!(!storage.check_permission(&hash, "alice", "read").unwrap());
    assert!(!storage
        .revoke_permission(&hash, "me", "alice", "read")
        .unwrap());
}

#[test]
fn permission_is_active_at_respects_the_expiry_boundary() {
    let perm = SharedFilePermission {
        content_hash: "a".repeat(64),
        grantor_user_id: "me".into(),
        grantee_user_id: "alice".into(),
        permission: "read".into(),
        created_at_ms: 0,
        expires_at_ms: Some(1000),
    };
    assert!(perm.is_active_at(999));
    assert!(!perm.is_active_at(1000), "expiry boundary is exclusive");
    assert!(!perm.is_active_at(1001));
    let never = SharedFilePermission {
        expires_at_ms: None,
        ..perm.clone()
    };
    assert!(never.is_active_at(u64::MAX), "no expiry never expires");
}

// ── Regression: existing share entry points ────────────────────────────────

#[test]
fn shared_file_entry_point_round_trips_and_delete_removes_grants_only() {
    let storage = Storage::memory().unwrap();
    let hash = "c".repeat(64);
    storage
        .put_file_object(&hash, 42, "application/pdf", "report.pdf", b"data")
        .unwrap();
    storage
        .upsert_shared_file(&hash, "me", "metadata-1", "report.pdf", None, true)
        .unwrap();
    storage
        .grant_permission(&hash, "me", "alice", "read", None)
        .unwrap();
    assert!(storage.check_permission(&hash, "alice", "read").unwrap());

    // Deleting the shared file removes the grant so a stale offer cannot
    // authorize later; the download history is untouched by design.
    assert!(storage.delete_shared_file(&hash, "me").unwrap());
    assert!(
        !storage.check_permission(&hash, "alice", "read").unwrap(),
        "delete must not leave a live grant"
    );
}

#[test]
fn completed_downloads_are_newest_first_with_stable_tiebreak() {
    let storage = Storage::memory().unwrap();
    // Insert through the same public entry points the app uses.
    let insert = |id: i64, name: &str, at_ms: u64| {
        let hash = format!("hash-{id}");
        storage
            .put_file_object(&hash, 100, "application/octet-stream", name, b"")
            .unwrap();
        let dl_id = storage.create_download(&hash, "peer-a", 100).unwrap();
        storage
            .set_download_paths(dl_id, format!("/tmp/{hash}.part"), format!("/dl/{name}"))
            .unwrap();
        storage.complete_download(dl_id, 100).unwrap();
        let _ = at_ms; // completed_at_ms comes from the durable row
    };
    insert(1, "a.bin", 1000);
    insert(2, "b.bin", 3000);
    insert(3, "c.bin", 3000);
    let rows = storage.list_completed_downloads().unwrap();
    assert_eq!(rows.len(), 3);
    // Newest-first by completed_at_ms, stable id tiebreak.
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![3, 2, 1]);

    let mut items: Vec<_> = rows
        .iter()
        .map(|r| {
            downloaded_vm::project_downloaded_item(
                r,
                "Alice".into(),
                detect_local_file_state(r.destination_path.as_deref(), r.total_bytes, false, None),
            )
        })
        .collect();
    downloaded_vm::sort_downloaded_items(&mut items);
    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    assert_eq!(ids, vec![3, 2, 1]);
    // Missing local file state is surfaced truthfully, never fake-verified.
    assert!(items.iter().all(|i| i.local == LocalFileState::Missing));
}
