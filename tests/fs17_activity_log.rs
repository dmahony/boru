//! FS-17 Activity Log view model — isolation harness.
//!
//! The Activity Log tab lives in the iced example crate, which is frequently
//! blocked from compiling by concurrent sibling work in the same workspace.
//! This harness includes the view model module via `#[path]` and compiles it
//! against the real `boru_core` lib so its unit tests can run independently.
//!
//! Run: `cargo test --test fs17_activity_log`

#[path = "../examples/iced_chat/activity_log_view_model.rs"]
mod activity_log_view_model;

use activity_log_view_model::{
    filter_activity_log, paginate_activity_log, project_activity_log, ActivityDirection,
    ActivityLogEnrichment, ActivityLogFilter, ActivityLogRow,
};
use boru_core::diagnostics::event_names;
use boru_core::storage::TransferActivityRow;

fn row(
    event_id: &str,
    transfer_id: &str,
    event_name: &str,
    occurred_at_ms: u64,
    direction: &str,
) -> TransferActivityRow {
    TransferActivityRow {
        event_id: event_id.into(),
        transfer_id: transfer_id.into(),
        event_name: event_name.into(),
        sequence: 0,
        occurred_at_ms,
        attempt: 1,
        payload_json: None,
        direction: direction.into(),
    }
}

#[test]
fn harness_projects_filters_and_paginates_a_mixed_history() {
    let rows = vec![
        row("e1", "t1", event_names::COMPLETION, 300, "inbound"),
        row("e2", "t2", event_names::COMPLETION, 200, "outbound"),
        row("e3", "t3", event_names::TRANSFER_STARTED, 100, "inbound"),
        row("e4", "t4", event_names::FAILURE, 50, "inbound"),
    ];
    let projected = project_activity_log(rows, &ActivityLogEnrichment::default());
    assert_eq!(projected.len(), 4);

    let by_others = filter_activity_log(&projected, ActivityLogFilter::ByOthers, "");
    assert_eq!(by_others.len(), 1);
    assert_eq!(by_others[0].direction, ActivityDirection::Outbound);
    assert_eq!(by_others[0].action, "Uploaded");

    let errors = filter_activity_log(&projected, ActivityLogFilter::Errors, "");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].action, "Failed");

    let page = paginate_activity_log(projected, 0, 3);
    assert_eq!(page.rows.len(), 3);
    assert_eq!(page.total, 4);
    assert_eq!(page.pages, 2);
}

#[test]
fn harness_large_history_stays_bounded_and_responsive() {
    let rows: Vec<TransferActivityRow> = (0..1000)
        .map(|i| {
            let name = if i % 10 == 0 {
                event_names::COMPLETION
            } else if i % 7 == 0 {
                event_names::FAILURE
            } else {
                event_names::PROGRESS_CHECKPOINT
            };
            row(&format!("e{i}"), &format!("t{i}"), name, i as u64, "inbound")
        })
        .collect();
    let projected = project_activity_log(rows, &ActivityLogEnrichment::default());
    assert_eq!(projected.len(), 1000);

    let errors = filter_activity_log(&projected, ActivityLogFilter::Errors, "");
    assert!(!errors.is_empty());

    let page = paginate_activity_log(filter_activity_log(&projected, ActivityLogFilter::All, ""), 0, 50);
    assert_eq!(page.rows.len(), 50);
    assert_eq!(page.pages, 20);
    assert!(page.has_next());
}

#[test]
fn harness_no_sensitive_material_in_debug_output() {
    let mut enrichment = ActivityLogEnrichment::default();
    enrichment.peer_labels.insert("t1".into(), "Alice".into());
    let rows = vec![row("e1", "t1", event_names::COMPLETION, 1, "inbound")];
    let projected = project_activity_log(rows, &enrichment);
    let debug = format!("{:?}", projected);
    assert!(!debug.contains('/'));
    assert!(!debug.contains("hash"));
    let _: Vec<ActivityLogRow> = projected;
}
