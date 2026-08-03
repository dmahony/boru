//! Full Activity Log tab projection (FS-17).
//!
//! This module translates the durable, privacy-filtered transfer activity
//! projection (`boru_core::storage::TransferActivityRow`) into stable, owned
//! UI rows for the Activity Log tab. It is deliberately independent of Iced
//! widgets and storage: the application layer feeds it rows plus a best-effort
//! enrichment map, and the module owns direction/outcome mapping, the
//! single-choice filter set, deterministic search, and pagination.
//!
//! Truthfulness rules (mirroring `docs/design/transfer-lifecycle-events.md`):
//! - Direction comes only from the projection's `direction` column, never from
//!   transfer-id prefixes or row resolvability — filters stay deterministic
//!   even after pruning.
//! - An action is derived from the recorded lifecycle event name, never from a
//!   guess. A request is shown as "Requested", not as success.
//! - Failure payloads expose only the closed `error_category` taxonomy; the
//!   UI maps `permission_denied` to "Denied" (grant refused or expired) and
//!   other categories to a generic "Failed".
//! - The details affordance carries a *bounded* raw detail built only from
//!   allow-listed payload keys (`error_category`, `reason`, `retry_delay_ms`,
//!   `duration_ms`). Paths, tokens, hashes, and arbitrary payload keys are
//!   discarded at the storage layer and never reconstructed here.
//! - Unknown future event names render as neutral notices.
//! - Removed/revoked items fall back to safe historical labels ("Shared item",
//!   "Remote peer") so a pruned row never breaks the list.

use std::collections::{HashMap, HashSet};

use boru_core::diagnostics::event_names;
use boru_core::storage::TransferActivityRow;

/// The storage projection is bounded to this many rows; the tab paginates over
/// it rather than growing without bound.
pub(crate) const STORAGE_ACTIVITY_LIMIT: usize = 1000;

/// Default rows per page in the Activity Log tab.
pub(crate) const ACTIVITY_LOG_PAGE_SIZE: usize = 50;

/// Maximum length of a raw detail string surfaced in the details affordance.
const MAX_RAW_DETAIL_CHARS: usize = 220;

/// Transfer direction, derived only from the durable projection's direction
/// column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActivityDirection {
    /// A remote peer sent bytes to this node — a download to me.
    Inbound,
    /// This node served bytes to a remote peer — an upload by others.
    Outbound,
    /// The stored direction value was neither "inbound" nor "outbound".
    Unknown,
}

impl ActivityDirection {
    pub(crate) fn from_str(value: &str) -> Self {
        match value {
            "inbound" => Self::Inbound,
            "outbound" => Self::Outbound,
            _ => Self::Unknown,
        }
    }

    /// Short accessible label rendered as real text.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Inbound => "To me",
            Self::Outbound => "By others",
            Self::Unknown => "Unknown",
        }
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Inbound => "inbound",
            Self::Outbound => "outbound",
            Self::Unknown => "unknown",
        }
    }
}

/// Truthful outcome category shown by the tab.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActivityOutcome {
    /// Transfer reached its successful terminal state.
    Success,
    /// Transfer failed or was denied/expired.
    Error,
    /// Transfer was cancelled, paused, or otherwise interrupted.
    Warning,
    /// Informational lifecycle point (request, start, progress, notice).
    Info,
}

impl ActivityOutcome {
    /// Short accessible label for the outcome; always rendered as real text so
    /// screen readers and colour-blind users get the same information as the
    /// status colour.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Success => "Completed",
            Self::Error => "Error",
            Self::Warning => "Attention",
            Self::Info => "Info",
        }
    }
}

/// The single-choice filter set for the Activity Log. Direction filters are
/// mutually exclusive with outcome filters, exactly like a segmented control;
/// the global search field composes on top of whichever filter is active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActivityLogFilter {
    All,
    /// Uploads / by others (outbound transfers we served).
    ByOthers,
    /// Downloads / to me (inbound transfers we received).
    ToMe,
    /// Transfers that reached a successful terminal state.
    Success,
    /// Transfers that are queued, authorised, transferring, verifying, or
    /// paused — anything that has not reached a terminal outcome yet.
    InProgress,
    /// Transfers that failed or were denied.
    Errors,
}

impl ActivityLogFilter {
    pub(crate) const ALL: [Self; 6] = [
        Self::All,
        Self::ByOthers,
        Self::ToMe,
        Self::Success,
        Self::InProgress,
        Self::Errors,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::ByOthers => "Uploads / By Others",
            Self::ToMe => "Downloads / To Me",
            Self::Success => "Success",
            Self::InProgress => "In Progress",
            Self::Errors => "Errors",
        }
    }
}

/// One durable activity row projected for the tab.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ActivityLogRow {
    /// Stable lifecycle event id — the deduplication key under replay.
    pub(crate) id: String,
    /// Short logical transfer id (used for enrichment; not rendered raw).
    pub(crate) transfer_id: String,
    /// Stable lifecycle event name from the closed taxonomy.
    pub(crate) event_name: String,
    /// Local observation timestamp in Unix milliseconds.
    pub(crate) occurred_at_ms: u64,
    /// Transfer direction (from the durable projection).
    pub(crate) direction: ActivityDirection,
    /// Peer display label (safe; never a raw public key or path).
    pub(crate) peer_label: String,
    /// File/folder display label (safe; never a local path or hash).
    pub(crate) file_label: String,
    /// Normalized action label (Requested, Authorized, Started, Downloaded,
    /// Uploaded, Failed, Cancelled, Denied, ...).
    pub(crate) action: String,
    /// Truthful outcome category.
    pub(crate) outcome: ActivityOutcome,
    /// Bounded, human-safe summary detail (failure category, percent, ...).
    pub(crate) detail: Option<String>,
    /// Bounded raw detail for the details affordance. Built only from
    /// allow-listed payload keys, so it cannot leak paths, tokens, or hashes.
    pub(crate) raw_detail: Option<String>,
    /// Optional byte count from the privacy-filtered payload.
    pub(crate) bytes: Option<u64>,
    /// Transfer attempt number.
    pub(crate) attempt: u32,
}

/// Best-effort display enrichment resolved by the application layer.
///
/// Keys are the opaque short `transfer_id` from the activity row. A missing
/// entry means the underlying download/file row was removed, pruned, or never
/// resolvable — the projection falls back to safe historical labels.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ActivityLogEnrichment {
    /// Short transfer id → peer display label.
    pub(crate) peer_labels: HashMap<String, String>,
    /// Short transfer id → file/folder display label.
    pub(crate) file_labels: HashMap<String, String>,
}

/// Map a lifecycle event name plus its privacy-filtered payload into a
/// truthful `(action, outcome, detail)` tuple. Raw error detail is produced
/// separately so the table can stay compact while the details affordance keeps
/// the bounded error context.
pub(crate) fn normalize_event(
    event_name: &str,
    payload_json: Option<&str>,
) -> (String, ActivityOutcome, Option<String>, Option<String>) {
    match event_name {
        event_names::DOWNLOAD_QUEUED => ("Queued".into(), ActivityOutcome::Info, None, None),
        event_names::ACCESS_REQUESTED => ("Requested".into(), ActivityOutcome::Info, None, None),
        event_names::ACCESS_GRANTED => ("Authorized".into(), ActivityOutcome::Info, None, None),
        event_names::TRANSFER_STARTED => ("Started".into(), ActivityOutcome::Info, None, None),
        event_names::PROGRESS_CHECKPOINT => {
            let detail = payload_percent(payload_json).map(|percent| format!("{percent}%"));
            ("In progress".into(), ActivityOutcome::Info, detail, None)
        }
        event_names::PAUSE => ("Paused".into(), ActivityOutcome::Warning, None, None),
        event_names::RESUME => ("Resumed".into(), ActivityOutcome::Info, None, None),
        event_names::VERIFICATION => ("Verifying".into(), ActivityOutcome::Info, None, None),
        // Completion action is direction-aware and applied by
        // `project_activity_log` ("Downloaded" vs "Uploaded").
        event_names::COMPLETION => ("Completed".into(), ActivityOutcome::Success, None, None),
        event_names::FAILURE => {
            let category = payload_category(payload_json, "error_category");
            let raw = raw_failure_detail(payload_json);
            match category.as_deref() {
                // The taxonomy maps both "refused" and "grant expired" onto
                // permission_denied; we surface it as Denied without inventing
                // which of the two it was.
                Some("permission_denied") => (
                    "Denied".into(),
                    ActivityOutcome::Error,
                    Some("permission denied or grant expired".into()),
                    raw,
                ),
                Some(other) => (
                    "Failed".into(),
                    ActivityOutcome::Error,
                    Some(other.replace('_', " ")),
                    raw,
                ),
                None => ("Failed".into(), ActivityOutcome::Error, None, raw),
            }
        }
        event_names::CANCELLATION => ("Cancelled".into(), ActivityOutcome::Warning, None, None),
        // Unknown future event names are preserved as neutral notices.
        _ => ("Activity".into(), ActivityOutcome::Info, None, None),
    }
}

/// True for lifecycle points that have not reached a terminal outcome.
///
/// Completion, failure, and cancellation are terminal; everything else
/// (queued, requested, granted, started, progress, pause, resume, verifying)
/// is still logically in progress.
pub(crate) fn is_in_progress(event_name: &str) -> bool {
    !matches!(
        event_name,
        event_names::COMPLETION | event_names::FAILURE | event_names::CANCELLATION
    )
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

/// Extract a bounded percent for progress rows. The payload key is named
/// `percent_millis` but stores the fraction scaled by 1,000,000 (percent ×
/// 10,000); divide by 10,000 and clamp to [0, 100].
fn payload_percent(payload_json: Option<&str>) -> Option<u64> {
    let value = serde_json::from_str::<serde_json::Value>(payload_json?).ok()?;
    value
        .get("percent_millis")
        .and_then(serde_json::Value::as_u64)
        .map(|parts_per_million| (parts_per_million / 10_000).min(100))
}

/// Extract a closed enum value from the privacy-filtered payload.
fn payload_category(payload_json: Option<&str>, key: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(payload_json?).ok()?;
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// Bounded raw failure context for the details affordance.
///
/// Only allow-listed payload keys are ever read (`error_category`, `reason`,
/// `retry_delay_ms`, `duration_ms`); the result is truncated to a fixed bound
/// so a pathological payload can never flood the UI.
fn raw_failure_detail(payload_json: Option<&str>) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(payload_json?).ok()?;
    let mut parts: Vec<String> = Vec::new();
    if let Some(category) = value
        .get("error_category")
        .and_then(serde_json::Value::as_str)
    {
        parts.push(format!("error_category={category}"));
    }
    if let Some(reason) = value.get("reason").and_then(serde_json::Value::as_str) {
        parts.push(format!("reason={}", &reason.chars().take(120).collect::<String>()));
    }
    if let Some(retry) = value.get("retry_delay_ms").and_then(serde_json::Value::as_u64) {
        parts.push(format!("retry_delay_ms={retry}"));
    }
    if let Some(duration) = value.get("duration_ms").and_then(serde_json::Value::as_u64) {
        parts.push(format!("duration_ms={duration}"));
    }
    if parts.is_empty() {
        return None;
    }
    let joined = parts.join(" · ");
    Some(joined.chars().take(MAX_RAW_DETAIL_CHARS).collect())
}

/// Project durable activity rows into tab rows: deduplicated by event id,
/// enriched with safe display labels, and ordered newest first (stable
/// tiebreak on event id for deterministic ordering under equal timestamps).
pub(crate) fn project_activity_log(
    rows: Vec<TransferActivityRow>,
    enrichment: &ActivityLogEnrichment,
) -> Vec<ActivityLogRow> {
    let mut seen = HashSet::with_capacity(rows.len());
    let mut projected = Vec::with_capacity(rows.len());

    for row in rows {
        // Belt-and-braces dedup: the SQLite projection already ignores
        // replayed event ids (INSERT OR IGNORE + PRIMARY KEY), but a caller
        // may feed a stream that was never persisted.
        if !seen.insert(row.event_id.clone()) {
            continue;
        }
        let direction = ActivityDirection::from_str(&row.direction);
        let (mut action, outcome, detail, raw_detail) =
            normalize_event(&row.event_name, row.payload_json.as_deref());
        // Completion is direction-aware: a download to me is "Downloaded", an
        // upload served to a remote peer is "Uploaded".
        if row.event_name == event_names::COMPLETION {
            action = match direction {
                ActivityDirection::Outbound => "Uploaded".to_string(),
                _ => "Downloaded".to_string(),
            };
        }
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
        projected.push(ActivityLogRow {
            id: row.event_id,
            transfer_id: row.transfer_id,
            event_name: row.event_name,
            occurred_at_ms: row.occurred_at_ms,
            direction,
            peer_label,
            file_label,
            action,
            outcome,
            detail,
            raw_detail,
            bytes: payload_bytes(row.payload_json.as_deref()),
            attempt: row.attempt,
        });
    }

    projected.sort_by(|a, b| {
        b.occurred_at_ms
            .cmp(&a.occurred_at_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
    projected
}

/// Apply the single-choice filter plus the global search query.
///
/// Deterministic: the filter is an exact predicate on the row's projected
/// direction/outcome, and search is a case-insensitive substring match over
/// the safe peer/file/action labels. Ordering (newest first, stable id
/// tiebreak) is preserved from the projection.
pub(crate) fn filter_activity_log(
    rows: &[ActivityLogRow],
    filter: ActivityLogFilter,
    query: &str,
) -> Vec<ActivityLogRow> {
    let query = query.trim().to_lowercase();
    rows.iter()
        .filter(|row| {
            let passes_filter = match filter {
                ActivityLogFilter::All => true,
                ActivityLogFilter::ByOthers => row.direction == ActivityDirection::Outbound,
                ActivityLogFilter::ToMe => row.direction == ActivityDirection::Inbound,
                ActivityLogFilter::Success => row.outcome == ActivityOutcome::Success,
                ActivityLogFilter::InProgress => is_in_progress(&row.event_name),
                ActivityLogFilter::Errors => row.outcome == ActivityOutcome::Error,
            };
            if !passes_filter {
                return false;
            }
            if query.is_empty() {
                return true;
            }
            row.peer_label.to_lowercase().contains(&query)
                || row.file_label.to_lowercase().contains(&query)
                || row.action.to_lowercase().contains(&query)
        })
        .cloned()
        .collect()
}

/// One page of the Activity Log table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ActivityLogPage {
    /// Rows on the current page (newest first).
    pub(crate) rows: Vec<ActivityLogRow>,
    /// Total rows matching the current filter/search.
    pub(crate) total: usize,
    /// Zero-based current page (clamped to a valid range).
    pub(crate) page: usize,
    /// Rows per page.
    pub(crate) page_size: usize,
    /// Total page count (at least 1).
    pub(crate) pages: usize,
}

impl ActivityLogPage {
    pub(crate) fn has_previous(&self) -> bool {
        self.page > 0
    }

    pub(crate) fn has_next(&self) -> bool {
        self.page + 1 < self.pages
    }
}

/// Split filtered rows into a deterministic page.
///
/// The page index is clamped to `[0, pages-1]`, so a stale page selection can
/// never render an empty page while rows exist.
pub(crate) fn paginate_activity_log(
    rows: Vec<ActivityLogRow>,
    page: usize,
    page_size: usize,
) -> ActivityLogPage {
    let page_size = page_size.max(1);
    let total = rows.len();
    let pages = if total == 0 {
        1
    } else {
        total.div_ceil(page_size)
    };
    let page = page.min(pages - 1);
    let start = page * page_size;
    let end = (start + page_size).min(total);
    let slice = if start < total {
        rows[start..end].to_vec()
    } else {
        Vec::new()
    };
    ActivityLogPage {
        rows: slice,
        total,
        page,
        page_size,
        pages,
    }
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
        direction: &str,
    ) -> TransferActivityRow {
        TransferActivityRow {
            event_id: event_id.into(),
            transfer_id: transfer_id.into(),
            event_name: event_name.into(),
            sequence: 0,
            occurred_at_ms,
            attempt: 1,
            payload_json: payload.map(str::to_owned),
            direction: direction.into(),
        }
    }

    fn row_in(
        event_id: &str,
        transfer_id: &str,
        event_name: &str,
        occurred_at_ms: u64,
    ) -> TransferActivityRow {
        row(event_id, transfer_id, event_name, occurred_at_ms, None, "inbound")
    }

    #[test]
    fn request_is_never_presented_as_success() {
        let (action, outcome, detail, raw) = normalize_event(event_names::ACCESS_REQUESTED, None);
        assert_eq!(action, "Requested");
        assert_eq!(outcome, ActivityOutcome::Info);
        assert!(detail.is_none());
        assert!(raw.is_none());

        let (_, outcome, _, _) = normalize_event(event_names::COMPLETION, None);
        assert_eq!(outcome, ActivityOutcome::Success);
    }

    #[test]
    fn lifecycle_stages_map_to_distinct_actions() {
        let cases = [
            (event_names::DOWNLOAD_QUEUED, "Queued"),
            (event_names::ACCESS_REQUESTED, "Requested"),
            (event_names::ACCESS_GRANTED, "Authorized"),
            (event_names::TRANSFER_STARTED, "Started"),
            (event_names::PROGRESS_CHECKPOINT, "In progress"),
            (event_names::VERIFICATION, "Verifying"),
            (event_names::CANCELLATION, "Cancelled"),
        ];
        for (name, expected) in cases {
            assert_eq!(normalize_event(name, None).0, expected, "{name}");
        }
    }

    #[test]
    fn permission_denied_failure_is_denied_not_generic_failed() {
        let payload = r#"{"error_category":"permission_denied","retryable":false}"#;
        let (action, outcome, detail, raw) = normalize_event(event_names::FAILURE, Some(payload));
        assert_eq!(action, "Denied");
        assert_eq!(outcome, ActivityOutcome::Error);
        assert_eq!(detail.as_deref(), Some("permission denied or grant expired"));
        assert!(raw.is_some());
    }

    #[test]
    fn other_failure_categories_map_to_failed_with_bounded_detail() {
        let payload = r#"{"error_category":"peer_unavailable","retryable":true}"#;
        let (action, outcome, detail, _) = normalize_event(event_names::FAILURE, Some(payload));
        assert_eq!(action, "Failed");
        assert_eq!(outcome, ActivityOutcome::Error);
        assert_eq!(detail.as_deref(), Some("peer unavailable"));
    }

    #[test]
    fn cancelled_and_paused_are_warnings() {
        assert_eq!(
            normalize_event(event_names::CANCELLATION, None).1,
            ActivityOutcome::Warning
        );
        assert_eq!(
            normalize_event(event_names::PAUSE, None).1,
            ActivityOutcome::Warning
        );
    }

    #[test]
    fn unknown_future_event_names_stay_neutral() {
        let (action, outcome, detail, raw) = normalize_event("future_event_v2", None);
        assert_eq!(action, "Activity");
        assert_eq!(outcome, ActivityOutcome::Info);
        assert!(detail.is_none());
        assert!(raw.is_none());
    }

    #[test]
    fn completion_action_is_direction_aware() {
        let inbound = row_in("evt-1", "1", event_names::COMPLETION, 100);
        let outbound = row(
            "evt-2",
            "2",
            event_names::COMPLETION,
            200,
            None,
            "outbound",
        );
        let projected = project_activity_log(vec![inbound, outbound], &ActivityLogEnrichment::default());
        assert_eq!(projected[0].action, "Uploaded");
        assert_eq!(projected[1].action, "Downloaded");
    }

    #[test]
    fn projection_is_newest_first_with_stable_tiebreak() {
        let rows = vec![
            row_in("evt-old", "1", event_names::COMPLETION, 100),
            row_in("evt-new", "2", event_names::ACCESS_REQUESTED, 300),
            row_in("evt-mid", "3", event_names::TRANSFER_STARTED, 200),
        ];
        let projected = project_activity_log(rows, &ActivityLogEnrichment::default());
        assert_eq!(
            projected.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["evt-new", "evt-mid", "evt-old"]
        );
    }

    #[test]
    fn projection_deduplicates_replayed_event_ids() {
        let rows = vec![
            row_in("evt-1", "1", event_names::COMPLETION, 100),
            row_in("evt-1", "1", event_names::COMPLETION, 100),
            row_in("evt-2", "2", event_names::FAILURE, 90),
        ];
        let projected = project_activity_log(rows, &ActivityLogEnrichment::default());
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].action, "Downloaded");
        assert_eq!(projected[1].action, "Failed");
    }

    #[test]
    fn direction_comes_from_the_storage_column_only() {
        let rows = vec![
            row_in("evt-1", "1", event_names::TRANSFER_STARTED, 100),
            row("evt-2", "2", event_names::TRANSFER_STARTED, 90, None, "outbound"),
            row("evt-3", "3", event_names::TRANSFER_STARTED, 80, None, "garbage"),
        ];
        let projected = project_activity_log(rows, &ActivityLogEnrichment::default());
        // Newest first (t=100 inbound, t=90 outbound, t=80 garbage).
        assert_eq!(projected[0].direction, ActivityDirection::Inbound);
        assert_eq!(projected[1].direction, ActivityDirection::Outbound);
        assert_eq!(projected[2].direction, ActivityDirection::Unknown);
    }

    #[test]
    fn filters_select_exact_direction_and_outcome_sets() {
        let rows = vec![
            row("evt-1", "1", event_names::COMPLETION, 100, None, "inbound"),
            row("evt-2", "2", event_names::COMPLETION, 90, None, "outbound"),
            row_in("evt-3", "3", event_names::TRANSFER_STARTED, 80),
            row_in("evt-4", "4", event_names::FAILURE, 70),
            row_in("evt-5", "5", event_names::CANCELLATION, 60),
        ];
        let projected = project_activity_log(rows, &ActivityLogEnrichment::default());

        let all = filter_activity_log(&projected, ActivityLogFilter::All, "");
        assert_eq!(all.len(), 5);

        let by_others = filter_activity_log(&projected, ActivityLogFilter::ByOthers, "");
        assert_eq!(by_others.len(), 1);
        assert_eq!(by_others[0].direction, ActivityDirection::Outbound);

        let to_me = filter_activity_log(&projected, ActivityLogFilter::ToMe, "");
        assert_eq!(to_me.len(), 4);
        assert!(to_me
            .iter()
            .all(|r| r.direction == ActivityDirection::Inbound));

        let success = filter_activity_log(&projected, ActivityLogFilter::Success, "");
        assert_eq!(success.len(), 2);

        let in_progress = filter_activity_log(&projected, ActivityLogFilter::InProgress, "");
        assert_eq!(in_progress.len(), 1);
        assert_eq!(in_progress[0].action, "Started");

        let errors = filter_activity_log(&projected, ActivityLogFilter::Errors, "");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].action, "Failed");
    }

    #[test]
    fn in_progress_excludes_all_terminal_outcomes() {
        for name in [
            event_names::COMPLETION,
            event_names::FAILURE,
            event_names::CANCELLATION,
        ] {
            assert!(!is_in_progress(name), "{name}");
        }
        for name in [
            event_names::DOWNLOAD_QUEUED,
            event_names::ACCESS_REQUESTED,
            event_names::ACCESS_GRANTED,
            event_names::TRANSFER_STARTED,
            event_names::PROGRESS_CHECKPOINT,
            event_names::PAUSE,
            event_names::RESUME,
            event_names::VERIFICATION,
        ] {
            assert!(is_in_progress(name), "{name}");
        }
    }

    #[test]
    fn combined_search_matches_peer_file_and_action_case_insensitively() {
        let mut enrichment = ActivityLogEnrichment::default();
        enrichment.peer_labels.insert("1".into(), "Alice".into());
        enrichment
            .file_labels
            .insert("1".into(), "report.pdf".into());
        let rows = vec![
            row_in("evt-1", "1", event_names::COMPLETION, 100),
            row_in("evt-2", "2", event_names::TRANSFER_STARTED, 90),
        ];
        let projected = project_activity_log(rows, &enrichment);

        // Peer match.
        let peer = filter_activity_log(&projected, ActivityLogFilter::All, "ALICE");
        assert_eq!(peer.len(), 1);
        assert_eq!(peer[0].peer_label, "Alice");
        // File match.
        let file = filter_activity_log(&projected, ActivityLogFilter::All, "report");
        assert_eq!(file.len(), 1);
        // Action match.
        let action = filter_activity_log(&projected, ActivityLogFilter::All, "started");
        assert_eq!(action.len(), 1);
        assert_eq!(action[0].action, "Started");
        // Filter + search compose.
        let composed = filter_activity_log(&projected, ActivityLogFilter::Success, "report");
        assert_eq!(composed.len(), 1);
        let empty = filter_activity_log(&projected, ActivityLogFilter::Success, "started");
        assert!(empty.is_empty());
    }

    #[test]
    fn pagination_clamps_and_pages_deterministically() {
        let rows: Vec<ActivityLogRow> = (0..137)
            .map(|i| ActivityLogRow {
                id: format!("evt-{i}"),
                transfer_id: format!("{i}"),
                event_name: event_names::PROGRESS_CHECKPOINT.into(),
                occurred_at_ms: i as u64,
                direction: ActivityDirection::Inbound,
                peer_label: "Remote peer".into(),
                file_label: "Shared item".into(),
                action: "In progress".into(),
                outcome: ActivityOutcome::Info,
                detail: None,
                raw_detail: None,
                bytes: None,
                attempt: 1,
            })
            .collect();

        let page0 = paginate_activity_log(rows.clone(), 0, 50);
        assert_eq!(page0.rows.len(), 50);
        assert_eq!(page0.total, 137);
        assert_eq!(page0.pages, 3);
        assert!(!page0.has_previous());
        assert!(page0.has_next());

        let page2 = paginate_activity_log(rows.clone(), 2, 50);
        assert_eq!(page2.rows.len(), 37);
        assert!(page2.has_previous());
        assert!(!page2.has_next());

        // Oversized page index clamps to the last page instead of showing empty.
        let clamped = paginate_activity_log(rows.clone(), 99, 50);
        assert_eq!(clamped.page, 2);
        assert_eq!(clamped.rows.len(), 37);

        // Empty input yields one empty page.
        let empty = paginate_activity_log(Vec::new(), 0, 50);
        assert_eq!(empty.pages, 1);
        assert!(empty.rows.is_empty());
        assert_eq!(empty.total, 0);
    }

    #[test]
    fn removed_items_fall_back_to_safe_historical_labels() {
        let rows = vec![row_in("evt-1", "42", event_names::COMPLETION, 100)];
        let projected = project_activity_log(rows, &ActivityLogEnrichment::default());
        assert_eq!(projected[0].peer_label, "Remote peer");
        assert_eq!(projected[0].file_label, "Shared item");
        assert!(!format!("{projected:?}").contains('/'));
        assert!(!format!("{projected:?}").contains("hash"));
    }

    #[test]
    fn enrichment_uses_resolved_labels_when_available() {
        let mut enrichment = ActivityLogEnrichment::default();
        enrichment.peer_labels.insert("42".into(), "Alice".into());
        enrichment
            .file_labels
            .insert("42".into(), "report.pdf".into());
        let rows = vec![row_in("evt-1", "42", event_names::COMPLETION, 100)];
        let projected = project_activity_log(rows, &enrichment);
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
            "inbound",
        )];
        let projected = project_activity_log(rows, &ActivityLogEnrichment::default());
        assert_eq!(projected[0].bytes, Some(1_048_576));
    }

    #[test]
    fn raw_detail_is_bounded_and_never_leaks_sensitive_keys() {
        // A hostile payload tries to smuggle a path and a token. The storage
        // allow-list would drop them, but the view model must also never read
        // them — raw detail is built only from the closed keys.
        let payload = r#"{"error_category":"timeout","reason":"peer went offline",
            "source_path":"/secret/private.txt","token":"do-not-store",
            "retry_delay_ms":5000,"duration_ms":1234}"#;
        let (_, _, _, raw) = normalize_event(event_names::FAILURE, Some(payload));
        let raw = raw.unwrap();
        assert!(raw.contains("error_category=timeout"));
        assert!(raw.contains("retry_delay_ms=5000"));
        assert!(!raw.contains("/secret"));
        assert!(!raw.contains("do-not-store"));
        assert!(raw.chars().count() <= MAX_RAW_DETAIL_CHARS);

        // Over-long reason is truncated to the bound.
        let long_reason = "x".repeat(400);
        let payload = format!(r#"{{"error_category":"timeout","reason":"{long_reason}"}}"#);
        let (_, _, _, raw) = normalize_event(event_names::FAILURE, Some(&payload));
        assert!(raw.unwrap().chars().count() <= MAX_RAW_DETAIL_CHARS);
    }

    #[test]
    fn progress_detail_reports_percent_without_fabrication() {
        let payload = r#"{"bytes_transferred":600,"total_bytes":1000,"percent_millis":600000}"#;
        let (action, _, detail, _) = normalize_event(event_names::PROGRESS_CHECKPOINT, Some(payload));
        assert_eq!(action, "In progress");
        assert_eq!(detail.as_deref(), Some("60%"));
    }

    #[test]
    fn filter_ordering_remains_newest_first() {
        let mut enrichment = ActivityLogEnrichment::default();
        enrichment.peer_labels.insert("1".into(), "Alice".into());
        enrichment.peer_labels.insert("2".into(), "Bob".into());
        let rows = vec![
            row_in("evt-old", "1", event_names::COMPLETION, 100),
            row_in("evt-new", "2", event_names::COMPLETION, 300),
        ];
        let projected = project_activity_log(rows, &enrichment);
        let success = filter_activity_log(&projected, ActivityLogFilter::Success, "");
        assert_eq!(
            success.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["evt-new", "evt-old"]
        );
    }
}
