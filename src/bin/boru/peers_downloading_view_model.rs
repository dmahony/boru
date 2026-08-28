#![allow(
    clippy::type_complexity,
    clippy::too_many_arguments,
    clippy::large_enum_variant,
    clippy::if_same_then_else,
    clippy::doc_lazy_continuation,
    clippy::doc_overindented_list_items,
    clippy::redundant_guards,
    clippy::manual_let_else,
    clippy::vec_init_then_push,
    clippy::let_underscore_future,
    clippy::needless_update,
    clippy::unnecessary_unwrap,
    clippy::single_match,
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::question_mark,
    clippy::unnecessary_sort_by,
    clippy::result_large_err,
    clippy::enum_variant_names,
    clippy::explicit_counter_loop,
    clippy::wrong_self_convention,
    missing_debug_implementations,
    unfulfilled_lint_expectations
)]
#![allow(dead_code)]

//! Outbound-transfer projection for the File Sharing dashboard (FS-11).
//!
//! This module translates the FS-05 live transfer projection records
//! (`TransferDirection::Outbound`) into stable, owned UI rows. It is
//! deliberately independent of Iced widgets and storage: the application
//! layer feeds it `TransferRecord`s plus a best-effort enrichment map and
//! the module owns ordering, truthful state/action mapping, and safe
//! formatting.
//!
//! Truthfulness rules (mirroring `docs/design/transfer-lifecycle-events.md`):
//! - A state label is derived 1:1 from the projection state; `Retrying` is
//!   only shown when the projection reports `attempt > 1`, never guessed.
//! - Progress is `Determinate` only when a positive total is known;
//!   otherwise an indeterminate bar plus a byte count is rendered and no
//!   percentage is fabricated.
//! - The peer label is the authenticated peer id string from the projection —
//!   the caller resolves it to a verified display identity; it is never read
//!   from an untrusted display field.
//! - The file label is a UI enrichment keyed by the stable item id (content
//!   hash) and falls back to a short hash prefix rather than a fabricated
//!   name or local path.

use std::collections::HashMap;

use boru_core::transfer_state_projection::{TransferRecord, TransferState};

/// Upper bound for retained finished outbound transfers (history view).
pub(crate) const MAX_OUTBOUND_HISTORY: usize = 50;

/// Truthful lifecycle state for an outbound transfer. Mapped 1:1 from the
/// FS-05 projection states; `Retrying` is derived from a real `attempt > 1`
/// on the active state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutboundState {
    /// Queued or actively transferring (the projection cannot distinguish
    /// admission from transfer — both are `Active`).
    Transferring,
    /// Integrity verification is running.
    Verifying,
    /// Finished successfully.
    Completed,
    /// Failed and no longer active.
    Failed,
    /// Cancelled by a user or lifecycle shutdown.
    Cancelled,
    /// Peer disconnected while the transfer was active.
    Disconnected,
    /// Same as `Transferring` but with `attempt > 1` — a genuine retry.
    Retrying,
}

impl OutboundState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Transferring => "Transferring",
            Self::Retrying => "Retrying",
            Self::Verifying => "Verifying",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Disconnected => "Disconnected",
        }
    }

    /// Finished states that leave the live list and move to history.
    /// Disconnected is intentionally not terminal — an interrupted transfer
    /// may resume when the peer returns.
    pub(crate) fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

impl From<TransferState> for OutboundState {
    fn from(state: TransferState) -> Self {
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

/// Progress is explicit about missing totals and missing observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutboundProgress {
    Determinate { bytes: u64, total: u64 },
    Indeterminate { bytes: u64 },
    Unknown,
}

impl OutboundProgress {
    pub(crate) fn from_bytes(bytes: u64, total: Option<u64>) -> Self {
        match total {
            Some(total) if total > 0 => Self::Determinate { bytes, total },
            _ => {
                if bytes > 0 {
                    Self::Indeterminate { bytes }
                } else {
                    Self::Unknown
                }
            }
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

/// A row for the "Peers Downloading from Me" panel, derived from one live
/// outbound transfer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PeersDownloadingRow {
    /// Stable transfer id (string form used by the projection).
    pub(crate) id: String,
    /// Authenticated peer id when known; the view resolves a display label
    /// and never trusts a remote-supplied display string.
    pub(crate) peer_id: Option<String>,
    /// Stable content/item id (content hash).
    pub(crate) item_id: String,
    /// Display name from the app's enrichment map; falls back to a short
    /// stable id prefix rather than a fabricated name or local path.
    pub(crate) display_name: String,
    /// Byte progress.
    pub(crate) progress: OutboundProgress,
    /// Truthful lifecycle state.
    pub(crate) state: OutboundState,
    /// Last accepted event timestamp.
    pub(crate) updated_at_ms: u64,
    /// Bounded error summary for failed transfers.
    pub(crate) error: Option<String>,
    /// Attempt number; > 1 means a genuine retry.
    pub(crate) attempt: u32,
}

/// Project a live outbound transfer record into a UI row.
///
/// `item_labels` maps `item_id` → display name (filled by the application
/// layer from the authenticated transfer context). Missing labels fall back
/// to a short item-id prefix — never a path or a fabricated name.
pub(crate) fn outbound_row(
    record: &TransferRecord,
    item_labels: &HashMap<String, String>,
) -> PeersDownloadingRow {
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
    PeersDownloadingRow {
        id: record.transfer_id.clone(),
        peer_id: record.peer_id.clone(),
        item_id: record.item_id.clone(),
        display_name,
        progress: OutboundProgress::from_bytes(record.bytes, record.total_bytes),
        state,
        updated_at_ms: record.updated_at_ms,
        error: record.error.clone(),
        attempt: record.attempt,
    }
}

/// Order outbound rows newest-first with a stable id tiebreaker.
pub(crate) fn sort_outbound_rows(rows: &mut [PeersDownloadingRow]) {
    rows.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
}

/// Apply a transfer record update to the active/history maps.
/// Returns `true` if the row was newly archived to history.
pub(crate) fn apply_outbound_update(
    record: &TransferRecord,
    active: &mut HashMap<String, TransferRecord>,
    history: &mut Vec<TransferRecord>,
) -> bool {
    if record.state.is_terminal() {
        let existed = active.remove(&record.transfer_id).is_some();
        let is_new = existed
            || !history
                .iter()
                .any(|existing| existing.transfer_id == record.transfer_id);
        if is_new {
            history.insert(0, record.clone());
            history.truncate(MAX_OUTBOUND_HISTORY);
        }
        is_new
    } else {
        history.retain(|existing| existing.transfer_id != record.transfer_id);
        active.insert(record.transfer_id.clone(), record.clone());
        false
    }
}

/// Rebuild active/history maps from a fresh projection snapshot.
pub(crate) fn resync_outbound_panel(
    snapshot: &[TransferRecord],
) -> (HashMap<String, TransferRecord>, Vec<TransferRecord>) {
    let mut active: HashMap<String, TransferRecord> = HashMap::new();
    let mut history: Vec<TransferRecord> = Vec::new();
    for record in snapshot {
        if record.state.is_terminal() {
            history.push(record.clone());
        } else {
            active.insert(record.transfer_id.clone(), record.clone());
        }
    }
    history.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| b.transfer_id.cmp(&a.transfer_id))
    });
    history.truncate(MAX_OUTBOUND_HISTORY);
    (active, history)
}

/// Human-readable byte formatting (kept local so the module is standalone).
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

/// Human-readable progress line: "1.2 MiB / 4.0 MiB", "1.2 MiB received",
/// or an explicit unknown marker. Never fabricates a percentage.
pub(crate) fn format_progress(progress: &OutboundProgress) -> String {
    match progress {
        OutboundProgress::Determinate { bytes, total } => {
            format!("{} / {}", format_bytes(*bytes), format_bytes(*total))
        }
        OutboundProgress::Indeterminate { bytes } => format!("{} received", format_bytes(*bytes)),
        OutboundProgress::Unknown => "Size unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boru_core::transfer_state_projection::{TransferDirection, TransferState};

    fn make_record(
        id: &str,
        state: TransferState,
        bytes: u64,
        total: Option<u64>,
        attempt: u32,
    ) -> TransferRecord {
        TransferRecord {
            transfer_id: id.to_string(),
            item_id: format!("hash_{id}"),
            direction: TransferDirection::Outbound,
            peer_id: Some("peer_abc".to_string()),
            bytes,
            total_bytes: total,
            state,
            started_at_ms: 1000,
            updated_at_ms: 2000,
            error: None,
            attempt,
        }
    }

    #[test]
    fn outbound_row_uses_item_label_when_present() {
        let record = make_record("t1", TransferState::Active, 500, Some(1000), 1);
        let labels: HashMap<String, String> =
            [("hash_t1".to_string(), "photo.jpg".to_string())].into();
        let row = outbound_row(&record, &labels);
        assert_eq!(row.display_name, "photo.jpg");
        assert_eq!(row.state, OutboundState::Transferring);
    }

    #[test]
    fn outbound_row_falls_back_to_hash_prefix() {
        let record = make_record("t2", TransferState::Active, 0, None, 1);
        let labels = HashMap::new();
        let row = outbound_row(&record, &labels);
        assert!(row.display_name.starts_with("file "));
    }

    #[test]
    fn outbound_row_detects_retry() {
        let record = make_record("t3", TransferState::Active, 100, Some(500), 2);
        let labels = HashMap::new();
        let row = outbound_row(&record, &labels);
        assert_eq!(row.state, OutboundState::Retrying);
    }

    #[test]
    fn outbound_row_terminal_states() {
        for (state, expected) in [
            (TransferState::Completed, OutboundState::Completed),
            (TransferState::Failed, OutboundState::Failed),
            (TransferState::Cancelled, OutboundState::Cancelled),
            (TransferState::Disconnected, OutboundState::Disconnected),
            (TransferState::Verifying, OutboundState::Verifying),
        ] {
            let record = make_record("t4", state, 0, None, 1);
            let row = outbound_row(&record, &HashMap::new());
            assert_eq!(row.state, expected, "state={state:?}");
        }
    }

    #[test]
    fn sort_outbound_rows_newest_first() {
        let mut rows = vec![
            PeersDownloadingRow {
                id: "a".into(),
                peer_id: None,
                item_id: "h1".into(),
                display_name: "a.txt".into(),
                progress: OutboundProgress::Unknown,
                state: OutboundState::Transferring,
                updated_at_ms: 1000,
                error: None,
                attempt: 1,
            },
            PeersDownloadingRow {
                id: "b".into(),
                peer_id: None,
                item_id: "h2".into(),
                display_name: "b.txt".into(),
                progress: OutboundProgress::Unknown,
                state: OutboundState::Transferring,
                updated_at_ms: 3000,
                error: None,
                attempt: 1,
            },
        ];
        sort_outbound_rows(&mut rows);
        assert_eq!(rows[0].id, "b");
        assert_eq!(rows[1].id, "a");
    }

    #[test]
    fn apply_outbound_update_archives_terminal() {
        let record = make_record("t5", TransferState::Completed, 1000, Some(1000), 1);
        let mut active: HashMap<String, TransferRecord> = HashMap::new();
        active.insert("t5".to_string(), record.clone());
        let mut history: Vec<TransferRecord> = Vec::new();
        let archived = apply_outbound_update(&record, &mut active, &mut history);
        assert!(archived);
        assert!(active.is_empty());
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].transfer_id, "t5");
    }

    #[test]
    fn apply_outbound_update_keeps_active() {
        let record = make_record("t6", TransferState::Active, 500, Some(1000), 1);
        let mut active: HashMap<String, TransferRecord> = HashMap::new();
        let mut history: Vec<TransferRecord> = Vec::new();
        let archived = apply_outbound_update(&record, &mut active, &mut history);
        assert!(!archived);
        assert_eq!(active.len(), 1);
        assert!(history.is_empty());
    }

    #[test]
    fn resync_outbound_panel_separates_active_and_terminal() {
        let active = make_record("a1", TransferState::Active, 100, Some(500), 1);
        let completed = make_record("c1", TransferState::Completed, 500, Some(500), 1);
        let failed = make_record("f1", TransferState::Failed, 0, None, 1);
        let snapshot = vec![active.clone(), completed.clone(), failed.clone()];
        let (active_map, history) = resync_outbound_panel(&snapshot);
        assert_eq!(active_map.len(), 1);
        assert!(active_map.contains_key("a1"));
        assert_eq!(history.len(), 2);
        // newest first: both have updated_at_ms=2000, so id tiebreaker
        assert_eq!(history[0].transfer_id, "f1");
        assert_eq!(history[1].transfer_id, "c1");
    }

    #[test]
    fn terminal_state_detection() {
        assert!(OutboundState::Completed.is_terminal());
        assert!(OutboundState::Failed.is_terminal());
        assert!(OutboundState::Cancelled.is_terminal());
        assert!(!OutboundState::Disconnected.is_terminal());
        assert!(!OutboundState::Transferring.is_terminal());
        assert!(!OutboundState::Retrying.is_terminal());
    }

    #[test]
    fn progress_formatting() {
        let det = OutboundProgress::Determinate {
            bytes: 1_048_576,
            total: 4_194_304,
        };
        assert!(format_progress(&det).contains("1.0 MiB / 4.0 MiB"));

        let indet = OutboundProgress::Indeterminate { bytes: 512_000 };
        assert!(format_progress(&indet).contains("received"));

        let unknown = OutboundProgress::Unknown;
        assert_eq!(format_progress(&unknown), "Size unknown");
    }
}
