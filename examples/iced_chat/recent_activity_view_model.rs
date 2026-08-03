//! Recent download activity projection for the File Sharing dashboard (FS-12).
//!
//! This module translates the durable, privacy-filtered transfer activity
//! projection (`boru_core::storage::TransferActivityRow`) into stable, owned
//! UI rows.  It is deliberately independent of Iced widgets and storage: the
//! application layer feeds it rows plus a best-effort enrichment map and the
//! module owns ordering, deduplication, and truthful action/status mapping.
//!
//! Truthfulness rules (mirroring `docs/design/transfer-lifecycle-events.md`):
//! - An action is derived from the recorded lifecycle event name, never from a
//!   guess.  A request is shown as "Requested", not as success.
//! - Failure payloads expose only the closed `error_category` taxonomy; the
//!   UI maps `permission_denied` to "Denied" (grant refused or expired) and
//!   other categories to a generic "Failed" with a bounded category detail.
//! - Unknown future event names render as neutral notices; they are never
//!   reinterpreted as success, failure, or cancellation.
//! - Removed/revoked items fall back to safe historical labels ("Shared item",
//!   "Remote peer") so a pruned row never breaks the list.

use std::collections::{HashMap, HashSet};

use boru_core::diagnostics::event_names;
use boru_core::storage::TransferActivityRow;

/// Upper bound for the dashboard card's recent subset (storage is bounded to
/// 1,000; the card shows a sensible recent window).
pub(crate) const MAX_RECENT_ACTIVITY_ROWS: usize = 50;

/// Truthful outcome category shown by the card.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActivityStatus {
    /// Transfer reached its successful terminal state.
    Success,
    /// Transfer failed or was denied/expired.
    Error,
    /// Transfer was cancelled, paused, or otherwise interrupted.
    Warning,
    /// Informational lifecycle point (request, start, progress, notice).
    Info,
}

impl ActivityStatus {
    /// Short accessible label for the status; always rendered as real text so
    /// screen readers and colour-blind users get the same information as the
    /// status icon colour.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Success => "Completed",
            Self::Error => "Error",
            Self::Warning => "Attention",
            Self::Info => "Info",
        }
    }
}

/// One row in the Recent Download Activity card.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecentActivityRow {
    /// Stable lifecycle event id — the deduplication key under replay.
    pub(crate) id: String,
    /// Local observation timestamp in Unix milliseconds.
    pub(crate) occurred_at_ms: u64,
    /// Peer display label (safe; never a raw public key or path).
    pub(crate) peer_label: String,
    /// File/folder display label (safe; never a local path or hash).
    pub(crate) file_label: String,
    /// Normalized action label (Requested, Authorized, Started, Downloaded,
    /// Failed, Cancelled, Denied, ...).
    pub(crate) action: String,
    /// Truthful outcome category.
    pub(crate) status: ActivityStatus,
    /// Optional bounded detail (e.g. failure category).
    pub(crate) detail: Option<String>,
    /// Optional byte count from the privacy-filtered payload.
    pub(crate) bytes: Option<u64>,
}

/// Best-effort display enrichment resolved by the application layer.
///
/// Keys are the opaque short `transfer_id` from the activity row.  A missing
/// entry means the underlying download/file row was removed, pruned, or never
/// resolvable — the projection falls back to safe historical labels.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ActivityEnrichment {
    /// Short transfer id → peer display label.
    pub(crate) peer_labels: HashMap<String, String>,
    /// Short transfer id → file/folder display label.
    pub(crate) file_labels: HashMap<String, String>,
}

/// Map a lifecycle event name plus its privacy-filtered payload into a
/// truthful `(action, status, detail)` tuple.
pub(crate) fn normalize_event(
    event_name: &str,
    payload_json: Option<&str>,
) -> (String, ActivityStatus, Option<String>) {
    match event_name {
        event_names::DOWNLOAD_QUEUED => ("Queued".into(), ActivityStatus::Info, None),
        event_names::ACCESS_REQUESTED => ("Requested".into(), ActivityStatus::Info, None),
        event_names::ACCESS_GRANTED => ("Authorized".into(), ActivityStatus::Info, None),
        event_names::TRANSFER_STARTED => ("Started".into(), ActivityStatus::Info, None),
        event_names::PROGRESS_CHECKPOINT => ("In progress".into(), ActivityStatus::Info, None),
        event_names::PAUSE => ("Paused".into(), ActivityStatus::Warning, None),
        event_names::RESUME => ("Resumed".into(), ActivityStatus::Info, None),
        event_names::VERIFICATION => ("Verifying".into(), ActivityStatus::Info, None),
        event_names::COMPLETION => ("Downloaded".into(), ActivityStatus::Success, None),
        event_names::FAILURE => {
            let category = payload_category(payload_json, "error_category");
            match category.as_deref() {
                // The taxonomy maps both "refused" and "grant expired" onto
                // permission_denied; we surface it as Denied without inventing
                // which of the two it was.
                Some("permission_denied") => (
                    "Denied".into(),
                    ActivityStatus::Error,
                    Some("permission denied or grant expired".into()),
                ),
                Some(other) => (
                    "Failed".into(),
                    ActivityStatus::Error,
                    Some(other.replace('_', " ")),
                ),
                None => ("Failed".into(), ActivityStatus::Error, None),
            }
        }
        event_names::CANCELLATION => ("Cancelled".into(), ActivityStatus::Warning, None),
        // Unknown future event names are preserved as neutral notices.
        _ => ("Activity".into(), ActivityStatus::Info, None),
    }
}

/// Extract a bounded payload counter (bytes_transferred / total_bytes) so the
/// row can show scale without exposing any sensitive field.
fn payload_bytes(payload_json: Option<&str>) -> Option<u64> {
    let value = serde_json::from_str::<serde_json::Value>(payload_json?).ok()?;
    for key in ["bytes_transferred", "total_bytes"] {
        if let Some(bytes) = value.get(key).and_then(serde_json::Value::as_u64) {
            return Some(bytes);
        }
    }
    None
}

/// Extract a closed enum value from the privacy-filtered payload.
fn payload_category(payload_json: Option<&str>, key: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(payload_json?).ok()?;
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// Project durable activity rows into card rows: deduplicated by event id,
/// enriched with safe display labels, and ordered newest first (stable
/// tiebreak on event id for deterministic ordering under equal timestamps).
pub(crate) fn project_recent_activity(
    rows: Vec<TransferActivityRow>,
    enrichment: &ActivityEnrichment,
) -> Vec<RecentActivityRow> {
    let mut seen = HashSet::with_capacity(rows.len());
    let mut projected = Vec::with_capacity(rows.len());

    for row in rows {
        // Belt-and-braces dedup: the SQLite projection already ignores
        // replayed event ids (INSERT OR IGNORE + PRIMARY KEY), but a caller
        // may feed a stream that was never persisted.
        if !seen.insert(row.event_id.clone()) {
            continue;
        }
        let (action, status, detail) =
            normalize_event(&row.event_name, row.payload_json.as_deref());
        let peer_label = enrichment
            .peer_labels
            .get(&row.transfer_id)
            .cloned()
            .unwrap_or_else(|| "Remote peer".to_string());
        let file_label = enrichment
            .file_labels
            .get(&row.transfer_id)
            .cloned()
            .unwrap_or_else(|| "Shared item".to_string());
        projected.push(RecentActivityRow {
            id: row.event_id,
            occurred_at_ms: row.occurred_at_ms,
            peer_label,
            file_label,
            action,
            status,
            detail,
            bytes: payload_bytes(row.payload_json.as_deref()),
        });
    }

    projected.sort_by(|a, b| {
        b.occurred_at_ms
            .cmp(&a.occurred_at_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
    projected.truncate(MAX_RECENT_ACTIVITY_ROWS);
    projected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        event_id: &str,
        transfer_id: &str,
        event_name: &str,
        occurred_at_ms: u64,
        payload: Option<&str>,
    ) -> TransferActivityRow {
        TransferActivityRow {
            event_id: event_id.into(),
            transfer_id: transfer_id.into(),
            event_name: event_name.into(),
            sequence: 0,
            occurred_at_ms,
            attempt: 1,
            payload_json: payload.map(str::to_owned),
        }
    }

    #[test]
    fn request_is_never_presented_as_success() {
        let (action, status, detail) = normalize_event(event_names::ACCESS_REQUESTED, None);
        assert_eq!(action, "Requested");
        assert_eq!(status, ActivityStatus::Info);
        assert!(detail.is_none());

        let (_, status, _) = normalize_event(event_names::COMPLETION, None);
        assert_eq!(status, ActivityStatus::Success);
    }

    #[test]
    fn lifecycle_stages_map_to_distinct_actions() {
        let cases = [
            (event_names::DOWNLOAD_QUEUED, "Queued"),
            (event_names::ACCESS_REQUESTED, "Requested"),
            (event_names::ACCESS_GRANTED, "Authorized"),
            (event_names::TRANSFER_STARTED, "Started"),
            (event_names::PROGRESS_CHECKPOINT, "In progress"),
            (event_names::COMPLETION, "Downloaded"),
            (event_names::CANCELLATION, "Cancelled"),
        ];
        for (name, expected) in cases {
            assert_eq!(normalize_event(name, None).0, expected, "{name}");
        }
    }

    #[test]
    fn permission_denied_failure_is_denied_not_generic_failed() {
        let payload = r#"{"error_category":"permission_denied","retryable":false}"#;
        let (action, status, detail) = normalize_event(event_names::FAILURE, Some(payload));
        assert_eq!(action, "Denied");
        assert_eq!(status, ActivityStatus::Error);
        assert_eq!(
            detail.as_deref(),
            Some("permission denied or grant expired")
        );
    }

    #[test]
    fn other_failure_categories_map_to_failed_with_bounded_detail() {
        let payload = r#"{"error_category":"peer_unavailable","retryable":true}"#;
        let (action, status, detail) = normalize_event(event_names::FAILURE, Some(payload));
        assert_eq!(action, "Failed");
        assert_eq!(status, ActivityStatus::Error);
        assert_eq!(detail.as_deref(), Some("peer unavailable"));
    }

    #[test]
    fn cancelled_and_paused_are_warnings() {
        assert_eq!(
            normalize_event(event_names::CANCELLATION, None).1,
            ActivityStatus::Warning
        );
        assert_eq!(
            normalize_event(event_names::PAUSE, None).1,
            ActivityStatus::Warning
        );
    }

    #[test]
    fn unknown_future_event_names_stay_neutral() {
        let (action, status, detail) = normalize_event("future_event_v2", None);
        assert_eq!(action, "Activity");
        assert_eq!(status, ActivityStatus::Info);
        assert!(detail.is_none());
    }

    #[test]
    fn projection_is_newest_first_with_stable_tiebreak() {
        let rows = vec![
            row("evt-old", "1", event_names::COMPLETION, 100, None),
            row("evt-new", "2", event_names::ACCESS_REQUESTED, 300, None),
            row("evt-mid", "3", event_names::TRANSFER_STARTED, 200, None),
        ];
        let projected = project_recent_activity(rows, &ActivityEnrichment::default());
        assert_eq!(
            projected.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["evt-new", "evt-mid", "evt-old"]
        );
    }

    #[test]
    fn projection_deduplicates_replayed_event_ids() {
        let rows = vec![
            row("evt-1", "1", event_names::COMPLETION, 100, None),
            row("evt-1", "1", event_names::COMPLETION, 100, None),
            row(
                "evt-2",
                "2",
                event_names::FAILURE,
                90,
                Some(r#"{"error_category":"timeout"}"#),
            ),
        ];
        let projected = project_recent_activity(rows, &ActivityEnrichment::default());
        // Replayed event id collapses to one row; newest first ordering puts
        // the completion (t=100) above the failure (t=90).
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].action, "Downloaded");
        assert_eq!(projected[1].action, "Failed");
    }

    #[test]
    fn removed_items_fall_back_to_safe_historical_labels() {
        let rows = vec![row("evt-1", "42", event_names::COMPLETION, 100, None)];
        let projected = project_recent_activity(rows, &ActivityEnrichment::default());
        assert_eq!(projected[0].peer_label, "Remote peer");
        assert_eq!(projected[0].file_label, "Shared item");
        assert!(!format!("{projected:?}").contains('/'));
        assert!(!format!("{projected:?}").contains("hash"));
    }

    #[test]
    fn enrichment_uses_resolved_labels_when_available() {
        let rows = vec![row("evt-1", "42", event_names::COMPLETION, 100, None)];
        let mut enrichment = ActivityEnrichment::default();
        enrichment.peer_labels.insert("42".into(), "Alice".into());
        enrichment
            .file_labels
            .insert("42".into(), "report.pdf".into());
        let projected = project_recent_activity(rows, &enrichment);
        assert_eq!(projected[0].peer_label, "Alice");
        assert_eq!(projected[0].file_label, "report.pdf");
    }

    #[test]
    fn byte_counts_come_only_from_allowed_payload_keys() {
        let rows = vec![row(
            "evt-1",
            "42",
            event_names::COMPLETION,
            100,
            Some(r#"{"bytes_transferred":1048576,"total_bytes":1048576}"#),
        )];
        let projected = project_recent_activity(rows, &ActivityEnrichment::default());
        assert_eq!(projected[0].bytes, Some(1_048_576));
    }

    #[test]
    fn card_subset_is_bounded() {
        let rows = (0..200)
            .map(|i| {
                row(
                    &format!("evt-{i}"),
                    &format!("{i}"),
                    event_names::PROGRESS_CHECKPOINT,
                    i as u64,
                    None,
                )
            })
            .collect();
        let projected = project_recent_activity(rows, &ActivityEnrichment::default());
        assert!(projected.len() <= MAX_RECENT_ACTIVITY_ROWS);
        assert_eq!(projected.len(), MAX_RECENT_ACTIVITY_ROWS);
    }

    #[test]
    fn status_labels_are_short_accessible_text() {
        assert_eq!(ActivityStatus::Success.label(), "Completed");
        assert_eq!(ActivityStatus::Error.label(), "Error");
        assert_eq!(ActivityStatus::Warning.label(), "Attention");
        assert_eq!(ActivityStatus::Info.label(), "Info");
    }
}
