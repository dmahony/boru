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

//! Incoming-transfer projection for the File Sharing dashboard (FS-14).
//!
//! This module translates the FS-05 live transfer projection records
//! (`TransferDirection::Inbound`) into stable, owned UI rows. It is
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
//! - Speed/ETA are only offered when they can be computed from real
//!   observation deltas (`bytes` and `updated_at_ms`), never extrapolated
//!   from a single sample.
//! - The destination summary is a local folder label; raw paths are never
//!   placed in rows (the view renders a safe summary and routes Open/Reveal
//!   through native helpers).

use std::collections::HashMap;

use boru_core::transfer_state_projection::{TransferRecord, TransferState};

/// Upper bound for retained finished inbound transfers (history view).
pub(crate) const MAX_INBOUND_HISTORY: usize = 50;

/// Truthful lifecycle state for an incoming transfer. Mapped 1:1 from the
/// FS-05 projection states; `Retrying` is derived from a real `attempt > 1`
/// on the active state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IncomingState {
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

impl IncomingState {
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

impl From<TransferState> for IncomingState {
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
pub(crate) enum IncomingProgress {
    Determinate { bytes: u64, total: u64 },
    Indeterminate { bytes: u64 },
    Unknown,
}

impl IncomingProgress {
    pub(crate) fn from_bytes(bytes: u64, total: Option<u64>) -> Self {
        match total {
            Some(total) if total > 0 => Self::Determinate { bytes, total },
            Some(_) if bytes > 0 => Self::Indeterminate { bytes },
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

/// A row for the Downloading tab, derived from one live inbound transfer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IncomingTransferRow {
    /// Stable transfer id (string form used by the projection).
    pub(crate) id: String,
    /// Authenticated peer id when known; the view resolves a display label
    /// and never trusts a remote-supplied display string.
    pub(crate) peer_id: Option<String>,
    /// Display name from the app's enrichment map; falls back to a short
    /// stable id prefix rather than a fabricated name or local path.
    pub(crate) display_name: String,
    /// Byte progress.
    pub(crate) progress: IncomingProgress,
    /// Truthful lifecycle state.
    pub(crate) state: IncomingState,
    /// First observed timestamp (ms since UNIX epoch).
    pub(crate) started_at_ms: u64,
    /// Last accepted event timestamp.
    pub(crate) updated_at_ms: u64,
    /// Bounded error summary for failed transfers.
    pub(crate) error: Option<String>,
    /// Attempt number; > 1 means a genuine retry.
    pub(crate) attempt: u32,
}

impl IncomingTransferRow {
    /// Speed in bytes/second when it can be computed from two real
    /// observations; `None` otherwise. `previous` must be the immediately
    /// preceding sample for the same transfer.
    pub(crate) fn speed_bps(&self, previous: Option<&IncomingTransferRow>) -> Option<u64> {
        let prev = previous?;
        if prev.id != self.id || prev.updated_at_ms >= self.updated_at_ms {
            return None;
        }
        let delta_bytes = self.bytes().saturating_sub(prev.bytes());
        let delta_ms = self.updated_at_ms.saturating_sub(prev.updated_at_ms);
        if delta_ms == 0 || delta_bytes == 0 {
            return None;
        }
        Some((delta_bytes as f64 / delta_ms as f64 * 1000.0).round() as u64)
    }

    /// Remaining seconds until completion, only when the total is known and
    /// speed is positive. Never fabricated for indeterminate transfers.
    pub(crate) fn eta_secs(&self, speed_bps: u64) -> Option<u64> {
        let IncomingProgress::Determinate { bytes, total } = self.progress else {
            return None;
        };
        if total <= bytes || speed_bps == 0 {
            return None;
        }
        Some((total - bytes).div_ceil(speed_bps))
    }

    fn bytes(&self) -> u64 {
        match self.progress {
            IncomingProgress::Determinate { bytes, .. }
            | IncomingProgress::Indeterminate { bytes } => bytes,
            IncomingProgress::Unknown => 0,
        }
    }
}

/// Project a live inbound transfer record into a UI row.
///
/// `item_labels` maps `item_id` → display name (filled by the application
/// layer from the authenticated transfer context). Missing labels fall back
/// to a short item-id prefix — never a path or a fabricated name.
pub(crate) fn incoming_row(
    record: &TransferRecord,
    item_labels: &HashMap<String, String>,
) -> IncomingTransferRow {
    let display_name = item_labels
        .get(&record.item_id)
        .cloned()
        .unwrap_or_else(|| {
            let prefix: String = record.item_id.chars().take(12).collect();
            format!("file {prefix}…")
        });
    let state = IncomingState::from(record.state);
    let state = if state == IncomingState::Transferring && record.attempt > 1 {
        IncomingState::Retrying
    } else {
        state
    };
    IncomingTransferRow {
        id: record.transfer_id.clone(),
        peer_id: record.peer_id.clone(),
        display_name,
        progress: IncomingProgress::from_bytes(record.bytes, record.total_bytes),
        state,
        started_at_ms: record.started_at_ms,
        updated_at_ms: record.updated_at_ms,
        error: record.error.clone(),
        attempt: record.attempt,
    }
}

/// Order incoming rows newest-first with a stable id tiebreaker.
pub(crate) fn sort_incoming_rows(rows: &mut [IncomingTransferRow]) {
    rows.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
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
pub(crate) fn format_progress(progress: &IncomingProgress) -> String {
    match progress {
        IncomingProgress::Determinate { bytes, total } => {
            format!("{} / {}", format_bytes(*bytes), format_bytes(*total))
        }
        IncomingProgress::Indeterminate { bytes } => format!("{} received", format_bytes(*bytes)),
        IncomingProgress::Unknown => "Size unknown".to_string(),
    }
}

/// Compact speed line, e.g. "1.2 MiB/s". Only called with a real speed.
pub(crate) fn format_speed(speed_bps: u64) -> String {
    format!("{}/s", format_bytes(speed_bps))
}

/// Compact ETA line, e.g. "ETA 45s" / "ETA 2m". Only called with a real ETA.
pub(crate) fn format_eta(eta_secs: u64) -> String {
    if eta_secs >= 3600 {
        format!("ETA {}h{}m", eta_secs / 3600, (eta_secs % 3600) / 60)
    } else if eta_secs >= 60 {
        format!("ETA {}m{}s", eta_secs / 60, eta_secs % 60)
    } else {
        format!("ETA {eta_secs}s")
    }
}

/// Started-time line using wall-clock minutes/hours, e.g. "started 14:05".
/// Falls back to a stable elapsed marker when the timestamp is missing/zero.
pub(crate) fn format_started(started_at_ms: u64, now_ms: u64) -> String {
    if started_at_ms == 0 {
        return "started unknown".to_string();
    }
    if now_ms <= started_at_ms {
        return "starting…".to_string();
    }
    let elapsed_secs = (now_ms - started_at_ms) / 1000;
    if elapsed_secs < 60 {
        format!("started {elapsed_secs}s ago")
    } else if elapsed_secs < 3600 {
        format!("started {}m ago", elapsed_secs / 60)
    } else if elapsed_secs < 86400 {
        format!("started {}h ago", elapsed_secs / 3600)
    } else {
        format!("started {}d ago", elapsed_secs / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boru_core::transfer_state_projection::{
        TransferDirection, TransferEvent, TransferProjection, TransferState,
    };

    fn record(
        transfer_id: &str,
        state: TransferState,
        bytes: u64,
        total: Option<u64>,
        attempt: u32,
        updated_at_ms: u64,
    ) -> TransferRecord {
        TransferRecord {
            transfer_id: transfer_id.to_string(),
            item_id: "hash-1234567890abcdef".to_string(),
            direction: TransferDirection::Inbound,
            peer_id: Some("peer-abc".to_string()),
            bytes,
            total_bytes: total,
            state,
            started_at_ms: 1000,
            updated_at_ms,
            error: None,
            attempt,
        }
    }

    #[test]
    fn incoming_row_maps_state_truthfully() {
        let mut labels = HashMap::new();
        labels.insert(
            "hash-1234567890abcdef".to_string(),
            "report.pdf".to_string(),
        );
        let row = incoming_row(
            &record("t1", TransferState::Active, 10, Some(100), 1, 2000),
            &labels,
        );
        assert_eq!(row.display_name, "report.pdf");
        assert_eq!(row.state, IncomingState::Transferring);
        assert_eq!(
            row.progress,
            IncomingProgress::Determinate {
                bytes: 10,
                total: 100
            }
        );
        assert_eq!(row.peer_id.as_deref(), Some("peer-abc"));
    }

    #[test]
    fn retrying_is_derived_from_real_attempt() {
        let labels = HashMap::new();
        let row = incoming_row(
            &record("t1", TransferState::Active, 10, Some(100), 2, 2000),
            &labels,
        );
        assert_eq!(row.state, IncomingState::Retrying);
        let row = incoming_row(
            &record("t1", TransferState::Active, 10, Some(100), 1, 2000),
            &labels,
        );
        assert_eq!(row.state, IncomingState::Transferring);
    }

    #[test]
    fn unknown_total_is_indeterminate_and_never_fake_percentage() {
        let row = IncomingProgress::from_bytes(10, None);
        assert_eq!(row, IncomingProgress::Indeterminate { bytes: 10 });
        assert_eq!(row.fraction(), None);
        let row = IncomingProgress::from_bytes(0, None);
        assert_eq!(row, IncomingProgress::Unknown);
        assert_eq!(row.fraction(), None);
    }

    #[test]
    fn speed_requires_two_real_observations() {
        let labels = HashMap::new();
        let prev = incoming_row(
            &record("t1", TransferState::Active, 100, Some(1000), 1, 1000),
            &labels,
        );
        let next = incoming_row(
            &record("t1", TransferState::Active, 200, Some(1000), 1, 2000),
            &labels,
        );
        assert_eq!(next.speed_bps(Some(&prev)), Some(100));
        // Same timestamp or same bytes → no speed.
        let same_time = incoming_row(
            &record("t1", TransferState::Active, 200, Some(1000), 1, 1000),
            &labels,
        );
        assert_eq!(same_time.speed_bps(Some(&prev)), None);
        let same_bytes = incoming_row(
            &record("t1", TransferState::Active, 100, Some(1000), 1, 2000),
            &labels,
        );
        assert_eq!(same_bytes.speed_bps(Some(&prev)), None);
        // Different transfer → no speed.
        let other = incoming_row(
            &record("t2", TransferState::Active, 200, Some(1000), 1, 2000),
            &labels,
        );
        assert_eq!(other.speed_bps(Some(&prev)), None);
        // No previous sample → no speed.
        assert_eq!(next.speed_bps(None), None);
    }

    #[test]
    fn eta_only_when_determinate_and_speed_positive() {
        let labels = HashMap::new();
        let row = incoming_row(
            &record("t1", TransferState::Active, 500, Some(1000), 1, 2000),
            &labels,
        );
        assert_eq!(row.eta_secs(100), Some(5));
        assert_eq!(row.eta_secs(0), None);
        let indeterminate = incoming_row(
            &record("t1", TransferState::Active, 500, None, 1, 2000),
            &labels,
        );
        assert_eq!(indeterminate.eta_secs(100), None);
        let complete = incoming_row(
            &record("t1", TransferState::Completed, 1000, Some(1000), 1, 2000),
            &labels,
        );
        assert_eq!(complete.eta_secs(100), None);
    }

    #[test]
    fn ordering_is_newest_first_with_stable_tiebreaker() {
        let labels = HashMap::new();
        let mut rows = vec![
            incoming_row(
                &record("b", TransferState::Active, 0, None, 1, 100),
                &labels,
            ),
            incoming_row(
                &record("a", TransferState::Active, 0, None, 1, 100),
                &labels,
            ),
            incoming_row(
                &record("c", TransferState::Active, 0, None, 1, 300),
                &labels,
            ),
        ];
        sort_incoming_rows(&mut rows);
        assert_eq!(
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["c", "b", "a"]
        );
    }

    #[test]
    fn formatting_is_byte_based_and_missing_data_is_explicit() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(
            format_progress(&IncomingProgress::Determinate {
                bytes: 1536,
                total: 3072
            }),
            "1.5 KiB / 3.0 KiB"
        );
        assert_eq!(
            format_progress(&IncomingProgress::Indeterminate { bytes: 10 }),
            "10 B received"
        );
        assert_eq!(format_progress(&IncomingProgress::Unknown), "Size unknown");
        assert_eq!(format_speed(1536), "1.5 KiB/s");
        assert_eq!(format_eta(45), "ETA 45s");
        assert_eq!(format_eta(125), "ETA 2m5s");
        assert_eq!(format_started(0, 10_000), "started unknown");
        assert_eq!(format_started(9_000, 10_000), "started 1s ago");
        assert_eq!(format_started(1000, 3_700_000), "started 1h ago");
    }

    #[test]
    fn projection_reducer_feeds_truthful_rows() {
        // Build the same lifecycle the app publishes: start → progress → done.
        let mut projection = TransferProjection::new(0);
        let ev = |event_id: &str,
                  seq: u64,
                  kind: boru_core::transfer_state_projection::EventName,
                  bytes: u64,
                  total: Option<u64>,
                  at: u64| TransferEvent {
            event_id: event_id.to_string(),
            transfer_id: "t1".to_string(),
            item_id: "hash-1234567890abcdef".to_string(),
            direction: TransferDirection::Inbound,
            peer_id: Some("peer-abc".to_string()),
            sequence: seq,
            attempt: 1,
            occurred_at_ms: at,
            kind,
            bytes,
            total_bytes: total,
            error: None,
        };
        projection.apply(ev(
            "s",
            0,
            boru_core::transfer_state_projection::EventName::Started,
            0,
            Some(100),
            1000,
        ));
        projection.apply(ev(
            "p1",
            1,
            boru_core::transfer_state_projection::EventName::Progress,
            40,
            Some(100),
            1500,
        ));
        projection.apply(ev(
            "p2",
            2,
            boru_core::transfer_state_projection::EventName::Progress,
            80,
            Some(100),
            2000,
        ));
        projection.apply(ev(
            "c",
            3,
            boru_core::transfer_state_projection::EventName::Completed,
            100,
            Some(100),
            2500,
        ));
        let snapshot = projection.snapshot();
        assert_eq!(snapshot.len(), 1);
        let labels = HashMap::new();
        let row = incoming_row(&snapshot[0], &labels);
        assert_eq!(row.state, IncomingState::Completed);
        assert_eq!(
            row.progress,
            IncomingProgress::Determinate {
                bytes: 100,
                total: 100
            }
        );
        assert_eq!(row.started_at_ms, 1000);
    }
}
