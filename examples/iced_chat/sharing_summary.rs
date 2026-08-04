//! FS-13 — "Sharing Summary" card for the File Sharing dashboard.
//!
//! Provides a small all-time overview of local sharing without expensive or
//! inconsistent counting. Every number rendered by this card is derived from
//! an authoritative durable store (SQLite rows) — never from the currently
//! rendered rows and never by scanning the filesystem.
//!
//! # Metric definitions (authoritative)
//!
//! - **Files shared** — number of distinct `shared_files` rows retained for
//!   the local profile (both currently offered and previously shared rows
//!   that are still retained). One row is one shared item; folders are not a
//!   shareable unit in this version — the native OS file picker selects
//!   files, and each indexed file is its own row. Stopping a share deletes
//!   the row, so the count reflects retained records (truthful "All time"
//!   scope for retained data).
//! - **Total downloads** — number of `downloads` rows in a terminal
//!   completed state (`complete` or `completed`). Completed transfers only;
//!   failed / cancelled / version-mismatch rows are NOT counted.
//! - **Active downloads** — number of `downloads` rows in a non-terminal
//!   state (queued, resolving_peer, requesting_permission, downloading,
//!   verifying, paused). This is the live count: it changes as the download
//!   state machine transitions rows.
//! - **Peers you've shared with** — number of distinct peers (hex-encoded
//!   public keys) the local profile has granted access to at least one
//!   shared file, from the `shared_file_permissions` table
//!   (`grantor_user_id` = local profile). Unique peers are identified by
//!   their `grantee_user_id`; the same peer granted access to several files
//!   counts once.
//!
//! # Scope label
//!
//! The card header reads "All time". This is truthful for retained data:
//! rows are retained until the user explicitly removes them (stop sharing /
//! delete history), so the numbers describe everything recorded so far.
//!
//! # Loading / unknown state
//!
//! While storage has not produced a projection yet, the card renders an
//! unknown state ("—") for every value instead of a premature zero, so a
//! flash of zero is never shown for data that has not loaded.

use boru_core::storage::{Download, SharedFileRow};

// ── Projection ──────────────────────────────────────────────────────────

/// The four Sharing Summary numbers, exactly as shown on the card.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SharingSummary {
    /// Distinct shared-files rows retained for the local profile.
    pub files_shared: u64,
    /// Completed downloads only (`complete` / `completed` terminal states).
    pub total_downloads: u64,
    /// Downloads currently in a non-terminal state (live count).
    pub active_downloads: u64,
    /// Distinct peers with an access grant from the local profile.
    pub peers_shared_with: u64,
}

/// States that count as a completed download for the summary metric.
///
/// The download state machine uses `complete` (verified + installed) and the
/// legacy `completed` alias. Failed, cancelled, and version-mismatch rows are
/// terminal outcomes but are deliberately NOT counted as downloads.
pub(crate) fn is_completed_download(state: &str) -> bool {
    matches!(state, "complete" | "completed")
}

/// States that count as an active (in-progress) download for the summary
/// metric. Paused downloads are still live — they can be resumed — so they
/// remain active until the user cancels them.
pub(crate) fn is_active_download(state: &str) -> bool {
    matches!(
        state,
        "queued"
            | "resolving_peer"
            | "requesting_permission"
            | "downloading"
            | "verifying"
            | "paused"
    )
}

/// Build the summary from authoritative rows.
///
/// - `shared_rows` come from `Storage::list_shared_files(profile, false)`
///   (all retained rows, offered or not).
/// - `downloads` come from `Storage::list_downloads()` (all states).
/// - `shared_peer_ids` come from `Storage::list_shared_peer_ids(profile)`.
///
/// The projection is a pure function of durable records, so counts always
/// match the underlying database and are trivially reproducible.
pub(crate) fn project_sharing_summary(
    shared_rows: &[SharedFileRow],
    downloads: &[Download],
    shared_peer_ids: &[String],
) -> SharingSummary {
    SharingSummary {
        files_shared: shared_rows.len() as u64,
        total_downloads: downloads
            .iter()
            .filter(|row| is_completed_download(&row.state))
            .count() as u64,
        active_downloads: downloads
            .iter()
            .filter(|row| is_active_download(&row.state))
            .count() as u64,
        peers_shared_with: shared_peer_ids.len() as u64,
    }
}

/// Format a metric value for the card. The unknown state is rendered as an
/// em dash so loading is visually distinct from a real zero.
pub(crate) fn format_value(value: Option<u64>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "—".to_string(),
    }
}

// ── View ────────────────────────────────────────────────────────────────

use iced::widget::{container, text, Column, Row, Space};
use iced::{Alignment, Length, Theme};

use crate::design_tokens;
use crate::fonts::Typography;

/// Card title — matches the FS-02 mockup's summary region.
const CARD_TITLE: &str = "Sharing Summary";
/// Truthful scope label for retained records.
const SCOPE_LABEL: &str = "All time";

/// Build the Sharing Summary card.
///
/// `summary == None` renders the loading/unknown state (em dashes) — never a
/// premature zero.
pub(crate) fn view_sharing_summary_card(
    summary: Option<SharingSummary>,
    theme: Theme,
) -> iced::Element<'static, crate::app::AppMessage> {
    let metrics = [
        ("FILES SHARED", summary.map(|value| value.files_shared)),
        (
            "TOTAL DOWNLOADS",
            summary.map(|value| value.total_downloads),
        ),
        (
            "ACTIVE DOWNLOADS",
            summary.map(|value| value.active_downloads),
        ),
        (
            "PEERS YOU'VE SHARED WITH",
            summary.map(|value| value.peers_shared_with),
        ),
    ];

    let mut grid = Column::new()
        .spacing(design_tokens::SPACE_12)
        .width(Length::Fill);
    for chunk in metrics.chunks(2) {
        let mut row = Row::new()
            .spacing(design_tokens::SPACE_12)
            .width(Length::Fill);
        for (label, value) in chunk {
            row = row.push(
                container(metric_cell(label, *value, &theme)).width(Length::FillPortion(1)),
            );
        }
        grid = grid.push(row);
    }

    // Header: title + truthful scope label.
    let header = Column::new()
        .push(
            text(CARD_TITLE)
                .size(Typography::SectionHeading.size_px())
                .font(Typography::SectionHeading.font())
                .color(design_tokens::text_primary(&theme)),
        )
        .push(
            text(SCOPE_LABEL)
                .size(Typography::SecondaryText.size_px())
                .font(Typography::SecondaryText.font())
                .color(design_tokens::text_muted(&theme)),
        )
        .spacing(design_tokens::SPACE_2)
        .width(Length::Fill);

    container(
        Column::new()
            .push(header)
            .push(Space::new().height(Length::Fixed(design_tokens::SPACE_12)))
            .push(grid)
            .spacing(0)
            .width(Length::Fill),
    )
    .padding([design_tokens::SPACE_16, design_tokens::SPACE_16])
    .width(Length::Fill)
    .style(|t| design_tokens::card_style(t))
    .into()
}

/// One metric cell: large value, small label — matching the mockup's
/// two-column summary layout.
fn metric_cell<'a>(
    label: &'a str,
    value: Option<u64>,
    theme: &Theme,
) -> iced::Element<'a, crate::app::AppMessage> {
    Column::new()
        .push(
            text(format_value(value))
                .font(Typography::PageTitle.font())
                .size(Typography::PageTitle.size_px())
                .color(design_tokens::text_primary(theme)),
        )
        .push(
            text(label)
                .size(Typography::SecondaryText.size_px())
                .font(Typography::SecondaryText.font())
                .color(design_tokens::text_muted(theme)),
        )
        .spacing(design_tokens::SPACE_2)
        .align_x(Alignment::Start)
        .into()
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use boru_core::storage::{Download, SharedFileRow};

    fn shared_row(hash: &str, name: &str) -> SharedFileRow {
        SharedFileRow {
            content_hash: hash.into(),
            profile_user_id: "local".into(),
            metadata_id: format!("meta-{hash}"),
            display_filename: name.into(),
            description: None,
            offered: true,
            created_at_ms: 10,
            updated_at_ms: 10,
            version: 1,
        }
    }

    fn download(id: i64, state: &str) -> Download {
        Download {
            id,
            content_hash: format!("hash-{id}"),
            remote_peer: "peer-a".into(),
            state: state.into(),
            bytes_downloaded: 0,
            total_bytes: 0,
            created_at_ms: 100,
            updated_at_ms: 100,
            last_error: None,
            retry_count: 0,
            next_retry_at_ms: None,
        }
    }

    #[test]
    fn completed_states_are_terminal_success_only() {
        assert!(is_completed_download("complete"));
        assert!(is_completed_download("completed"));
        for state in [
            "queued",
            "downloading",
            "verifying",
            "paused",
            "failed",
            "cancelled",
            "version_mismatch",
        ] {
            assert!(
                !is_completed_download(state),
                "{state} must not count as completed"
            );
        }
    }

    #[test]
    fn active_states_exclude_terminal_outcomes() {
        for state in [
            "queued",
            "resolving_peer",
            "requesting_permission",
            "downloading",
            "verifying",
            "paused",
        ] {
            assert!(is_active_download(state), "{state} must count as active");
        }
        for state in [
            "complete",
            "completed",
            "failed",
            "cancelled",
            "version_mismatch",
        ] {
            assert!(
                !is_active_download(state),
                "{state} must not count as active"
            );
        }
    }

    #[test]
    fn projection_counts_match_underlying_records() {
        let shared = vec![
            shared_row("a", "a.txt"),
            shared_row("b", "b.txt"),
            shared_row("c", "c.txt"),
        ];
        let downloads = vec![
            download(1, "complete"),
            download(2, "completed"),
            download(3, "downloading"),
            download(4, "paused"),
            download(5, "failed"),
            download(6, "queued"),
        ];
        let peers = vec![
            "peer-a".to_string(),
            "peer-b".to_string(),
            "peer-a".to_string(),
        ];

        // list_shared_peer_ids returns DISTINCT ids, so the caller supplies a
        // deduplicated list; the projection counts what it is given.
        let distinct_peers = {
            let mut seen = std::collections::BTreeSet::new();
            peers
                .into_iter()
                .filter(|p| seen.insert(p.clone()))
                .collect::<Vec<_>>()
        };

        let summary = project_sharing_summary(&shared, &downloads, &distinct_peers);
        assert_eq!(summary.files_shared, 3);
        assert_eq!(summary.total_downloads, 2);
        assert_eq!(summary.active_downloads, 3);
        assert_eq!(summary.peers_shared_with, 2);
    }

    #[test]
    fn empty_records_project_to_zero_but_are_a_real_value() {
        let summary = project_sharing_summary(&[], &[], &[]);
        assert_eq!(summary, SharingSummary::default());
        // A loaded zero is distinguishable from the unknown state.
        assert_eq!(format_value(Some(0)), "0");
        assert_eq!(format_value(None), "—");
    }

    #[test]
    fn one_folder_of_files_counts_as_one_row_per_file() {
        // Folders are not a shareable unit: the native picker selects files,
        // and each indexed file is its own shared_files row. A folder with
        // two files therefore contributes two rows, not one.
        let shared = vec![
            shared_row("f1", "dir/file1.txt"),
            shared_row("f2", "dir/file2.txt"),
        ];
        let summary = project_sharing_summary(&shared, &[], &[]);
        assert_eq!(summary.files_shared, 2);
    }

    #[test]
    fn card_builds_without_panic_for_unknown_and_loaded_states() {
        let el = view_sharing_summary_card(None, Theme::Light);
        let _ = el;
        let summary = SharingSummary {
            files_shared: 7,
            total_downloads: 3,
            active_downloads: 1,
            peers_shared_with: 2,
        };
        let el = view_sharing_summary_card(Some(summary), Theme::Dark);
        let _ = el;
    }

    #[test]
    fn format_value_is_deterministic() {
        assert_eq!(format_value(Some(12)), "12");
        assert_eq!(format_value(Some(0)), "0");
        assert_eq!(format_value(None), "—");
    }
}
