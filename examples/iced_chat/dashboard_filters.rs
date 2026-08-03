//! FS-18 — dashboard search normalization, per-tab matching, and sort controls.
//!
//! This module is deliberately independent of Iced widgets and storage. It
//! owns:
//!
//! - **Search semantics.** One normalized query string is preserved across all
//!   five dashboard tabs; each tab interprets it against its own relevant
//!   fields (file display names, peer display identity, and — where the raw
//!   peer id is available — its short visible prefix). Matching is a
//!   case-insensitive, Unicode-aware substring test over trimmed text, so it
//!   stays immediate for in-memory buffers (no debounce needed).
//!
//! - **Sort controls.** Deterministic comparators with a stable id tiebreaker
//!   for the three tabs that benefit from sorting: Shared by Me
//!   (name / date shared / size / downloads), Downloaded
//!   (completed time / name / size), and Activity (time / status). Every sort
//!   is a `(key, descending)` pair; clicking a key toggles direction, clicking
//!   a different key switches to that key's default direction.
//!
//! - **State retention policy.** Sort state is a plain value the application
//!   stores on its screen state, exactly like the active tab and the search
//!   query, so all three survive in-session navigation away from and back to
//!   the dashboard. It is never persisted to disk, and it never mutates the
//!   authoritative row buffers: the app clones/filters, then sorts the
//!   filtered copy, leaving storage projections and summary metrics intact.
//!
//! Privacy: matching runs against display labels and public-key ids only; no
//! local path, content hash, or raw payload is ever used as a search haystack,
//! and nothing here renders text.

use crate::activity_log_view_model::{ActivityLogRow, ActivityOutcome};
use crate::dashboard_view_model::CompletedDownloadItem;
use crate::shared_by_me_table::SharedByMeRow;

/// Normalize text for matching: trim surrounding whitespace and lowercase
/// using full Unicode rules. Deterministic and locale-independent.
pub(crate) fn normalize(text: &str) -> String {
    text.trim().to_lowercase()
}

/// True when the (normalized) query is a substring of any normalized haystack.
///
/// An empty or whitespace-only query matches everything, so callers can pass
/// the raw header query directly without branching first.
pub(crate) fn query_matches(query: &str, haystacks: &[&str]) -> bool {
    let query = normalize(query);
    if query.is_empty() {
        return true;
    }
    haystacks.iter().any(|haystack| normalize(haystack).contains(&query))
}

/// Short visible peer-id prefix used as an extra search haystack.
///
/// The recipient id carried by projections is the full hex public key; typing
/// the short prefix (what `PublicKey::fmt_short` renders) already matches via
/// substring, so this helper exists mainly for callers that only hold a full
/// id string and want to be explicit about the short form being searchable.
pub(crate) fn short_peer_id(id: &str) -> String {
    id.chars().take(8).collect()
}

// ── Shared by Me sort ─────────────────────────────────────────────────

/// Sortable keys on the Shared by Me table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SharedByMeSortKey {
    Name,
    DateShared,
    Size,
    Downloads,
}

impl SharedByMeSortKey {
    pub(crate) const ALL: [Self; 4] = [
        Self::DateShared,
        Self::Name,
        Self::Size,
        Self::Downloads,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::DateShared => "Date shared",
            Self::Size => "Size",
            Self::Downloads => "Downloads",
        }
    }

    /// Default direction when this key becomes the active sort: newest/largest
    /// first for date/size/downloads, alphabetical for name.
    fn default_descending(self) -> bool {
        !matches!(self, Self::Name)
    }
}

/// Active sort for the Shared by Me table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SharedByMeSort {
    pub(crate) key: SharedByMeSortKey,
    pub(crate) descending: bool,
}

impl Default for SharedByMeSort {
    fn default() -> Self {
        Self {
            key: SharedByMeSortKey::DateShared,
            descending: true,
        }
    }
}

impl SharedByMeSort {
    /// Next sort state after the user clicks `key`: toggle direction when it
    /// is already active, otherwise adopt the key's default direction.
    pub(crate) fn on_key_clicked(&self, key: SharedByMeSortKey) -> Self {
        if self.key == key {
            Self {
                key,
                descending: !self.descending,
            }
        } else {
            Self {
                key,
                descending: key.default_descending(),
            }
        }
    }

    /// Sort rows in place. Every comparator ends with a stable row-id
    /// tiebreaker (never reversed), so equal keys keep a canonical order.
    pub(crate) fn apply(self, rows: &mut [SharedByMeRow]) {
        match self.key {
            SharedByMeSortKey::Name => {
                if self.descending {
                    rows.sort_by(|a, b| {
                        normalize(&b.display_name)
                            .cmp(&normalize(&a.display_name))
                            .then_with(|| a.id.cmp(&b.id))
                    });
                } else {
                    rows.sort_by(|a, b| {
                        normalize(&a.display_name)
                            .cmp(&normalize(&b.display_name))
                            .then_with(|| a.id.cmp(&b.id))
                    });
                }
            }
            SharedByMeSortKey::DateShared => {
                if self.descending {
                    rows.sort_by(|a, b| {
                        b.shared_on_ms
                            .cmp(&a.shared_on_ms)
                            .then_with(|| a.id.cmp(&b.id))
                    });
                } else {
                    rows.sort_by(|a, b| {
                        a.shared_on_ms
                            .cmp(&b.shared_on_ms)
                            .then_with(|| a.id.cmp(&b.id))
                    });
                }
            }
            SharedByMeSortKey::Size => sort_optional_u64(
                rows,
                self.descending,
                |row| row.size_bytes,
                |row| row.id.clone(),
            ),
            SharedByMeSortKey::Downloads => sort_optional_u64(
                rows,
                self.descending,
                |row| row.downloads,
                |row| row.id.clone(),
            ),
        }
    }
}

/// Sort a slice by an optional `u64` key (missing values sort before present
/// ones in ascending order; the direction reverses that), with a stable id
/// tiebreaker.
fn sort_optional_u64<T, F, G>(items: &mut [T], descending: bool, key: F, id: G)
where
    F: Fn(&T) -> Option<u64>,
    G: Fn(&T) -> String,
{
    if descending {
        items.sort_by(|a, b| {
            key(b)
                .cmp(&key(a))
                .then_with(|| id(a).cmp(&id(b)))
        });
    } else {
        items.sort_by(|a, b| {
            key(a)
                .cmp(&key(b))
                .then_with(|| id(a).cmp(&id(b)))
        });
    }
}

// ── Downloaded sort ───────────────────────────────────────────────────

/// Sortable keys on the Downloaded tab.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DownloadedSortKey {
    CompletedTime,
    Name,
    Size,
}

impl DownloadedSortKey {
    pub(crate) const ALL: [Self; 3] = [Self::CompletedTime, Self::Name, Self::Size];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::CompletedTime => "Completed",
            Self::Name => "Name",
            Self::Size => "Size",
        }
    }

    fn default_descending(self) -> bool {
        !matches!(self, Self::Name)
    }
}

/// Active sort for the Downloaded tab.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DownloadedSort {
    pub(crate) key: DownloadedSortKey,
    pub(crate) descending: bool,
}

impl Default for DownloadedSort {
    fn default() -> Self {
        Self {
            key: DownloadedSortKey::CompletedTime,
            descending: true,
        }
    }
}

impl DownloadedSort {
    pub(crate) fn on_key_clicked(&self, key: DownloadedSortKey) -> Self {
        if self.key == key {
            Self {
                key,
                descending: !self.descending,
            }
        } else {
            Self {
                key,
                descending: key.default_descending(),
            }
        }
    }

    pub(crate) fn apply(self, items: &mut [CompletedDownloadItem]) {
        match self.key {
            DownloadedSortKey::CompletedTime => {
                if self.descending {
                    items.sort_by(|a, b| {
                        b.completed_at_ms
                            .cmp(&a.completed_at_ms)
                            .then_with(|| a.id.cmp(&b.id))
                    });
                } else {
                    items.sort_by(|a, b| {
                        a.completed_at_ms
                            .cmp(&b.completed_at_ms)
                            .then_with(|| a.id.cmp(&b.id))
                    });
                }
            }
            DownloadedSortKey::Name => {
                if self.descending {
                    items.sort_by(|a, b| {
                        normalize(&b.display_name)
                            .cmp(&normalize(&a.display_name))
                            .then_with(|| a.id.cmp(&b.id))
                    });
                } else {
                    items.sort_by(|a, b| {
                        normalize(&a.display_name)
                            .cmp(&normalize(&b.display_name))
                            .then_with(|| a.id.cmp(&b.id))
                    });
                }
            }
            DownloadedSortKey::Size => {
                if self.descending {
                    items.sort_by(|a, b| {
                        b.size_bytes
                            .cmp(&a.size_bytes)
                            .then_with(|| a.id.cmp(&b.id))
                    });
                } else {
                    items.sort_by(|a, b| {
                        a.size_bytes
                            .cmp(&b.size_bytes)
                            .then_with(|| a.id.cmp(&b.id))
                    });
                }
            }
        }
    }

    /// Sort a slice of row *references* in place, with identical deterministic
    /// comparators to [`DownloadedSort::apply`]. Used by the view when it holds
    /// borrows into its authoritative history buffer so no copies are made and
    /// the authoritative data is never mutated.
    pub(crate) fn apply_ref<'a>(self, items: &mut [&'a CompletedDownloadItem]) {
        match self.key {
            DownloadedSortKey::CompletedTime => {
                if self.descending {
                    items.sort_by(|a, b| {
                        b.completed_at_ms
                            .cmp(&a.completed_at_ms)
                            .then_with(|| a.id.cmp(&b.id))
                    });
                } else {
                    items.sort_by(|a, b| {
                        a.completed_at_ms
                            .cmp(&b.completed_at_ms)
                            .then_with(|| a.id.cmp(&b.id))
                    });
                }
            }
            DownloadedSortKey::Name => {
                if self.descending {
                    items.sort_by(|a, b| {
                        normalize(&b.display_name)
                            .cmp(&normalize(&a.display_name))
                            .then_with(|| a.id.cmp(&b.id))
                    });
                } else {
                    items.sort_by(|a, b| {
                        normalize(&a.display_name)
                            .cmp(&normalize(&b.display_name))
                            .then_with(|| a.id.cmp(&b.id))
                    });
                }
            }
            DownloadedSortKey::Size => {
                if self.descending {
                    items.sort_by(|a, b| {
                        b.size_bytes
                            .cmp(&a.size_bytes)
                            .then_with(|| a.id.cmp(&b.id))
                    });
                } else {
                    items.sort_by(|a, b| {
                        a.size_bytes
                            .cmp(&b.size_bytes)
                            .then_with(|| a.id.cmp(&b.id))
                    });
                }
            }
        }
    }
}

// ── Activity sort ─────────────────────────────────────────────────────

/// Sortable keys on the Activity Log tab.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActivitySortKey {
    Time,
    Status,
}

impl ActivitySortKey {
    pub(crate) const ALL: [Self; 2] = [Self::Time, Self::Status];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Time => "Time",
            Self::Status => "Status",
        }
    }

    fn default_descending(self) -> bool {
        // Newest first and most-severe status first are the natural defaults.
        true
    }
}

/// Active sort for the Activity Log tab.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ActivitySort {
    pub(crate) key: ActivitySortKey,
    pub(crate) descending: bool,
}

impl Default for ActivitySort {
    fn default() -> Self {
        Self {
            key: ActivitySortKey::Time,
            descending: true,
        }
    }
}

impl ActivitySort {
    pub(crate) fn on_key_clicked(&self, key: ActivitySortKey) -> Self {
        if self.key == key {
            Self {
                key,
                descending: !self.descending,
            }
        } else {
            Self {
                key,
                descending: key.default_descending(),
            }
        }
    }

    pub(crate) fn apply(self, rows: &mut [ActivityLogRow]) {
        match self.key {
            ActivitySortKey::Time => {
                if self.descending {
                    rows.sort_by(|a, b| {
                        b.occurred_at_ms
                            .cmp(&a.occurred_at_ms)
                            .then_with(|| a.id.cmp(&b.id))
                    });
                } else {
                    rows.sort_by(|a, b| {
                        a.occurred_at_ms
                            .cmp(&b.occurred_at_ms)
                            .then_with(|| a.id.cmp(&b.id))
                    });
                }
            }
            ActivitySortKey::Status => {
                if self.descending {
                    rows.sort_by(|a, b| {
                        outcome_rank(a.outcome)
                            .cmp(&outcome_rank(b.outcome))
                            .then_with(|| b.occurred_at_ms.cmp(&a.occurred_at_ms))
                            .then_with(|| a.id.cmp(&b.id))
                    });
                } else {
                    rows.sort_by(|a, b| {
                        outcome_rank(b.outcome)
                            .cmp(&outcome_rank(a.outcome))
                            .then_with(|| b.occurred_at_ms.cmp(&a.occurred_at_ms))
                            .then_with(|| a.id.cmp(&b.id))
                    });
                }
            }
        }
    }
}

/// Total order over outcomes for status sorting: most severe first.
fn outcome_rank(outcome: ActivityOutcome) -> u8 {
    match outcome {
        ActivityOutcome::Error => 0,
        ActivityOutcome::Warning => 1,
        ActivityOutcome::Success => 2,
        ActivityOutcome::Info => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity_log_view_model::ActivityLogRow;
    use crate::dashboard_view_model::{project_completed_download, LocalFileState};
    use crate::shared_by_me_table::{RecipientAccess, RecipientView};

    fn shared_row(id: &str, name: &str, shared_on_ms: u64, size: Option<u64>) -> SharedByMeRow {
        SharedByMeRow {
            id: id.into(),
            content_hash: format!("hash-{id}"),
            display_name: name.into(),
            mime_type: Some("text/plain".into()),
            size_bytes: size,
            shared_on_ms,
            recipients: vec![RecipientView {
                id: format!("peer-{id}"),
                label: format!("Peer {id}"),
                access: RecipientAccess::Allowed,
            }],
            has_explicit_recipients: true,
            source_available: true,
            downloads: None,
        }
    }

    fn completed(id: i64, name: &str, at: u64, size: u64) -> CompletedDownloadItem {
        let record = boru_core::storage::CompletedDownloadRecord {
            id,
            content_hash: format!("hash-{id}"),
            remote_peer: format!("peer-{id}"),
            total_bytes: size,
            completed_at_ms: at,
            destination_path: None,
            display_filename: name.into(),
            mime_type: "application/octet-stream".into(),
        };
        let peer_label = format!("Peer {id}");
        project_completed_download(&record, &peer_label, LocalFileState::Verified)
    }

    fn activity(id: &str, at: u64, outcome: ActivityOutcome) -> ActivityLogRow {
        ActivityLogRow {
            id: id.into(),
            transfer_id: id.into(),
            event_name: "event".into(),
            occurred_at_ms: at,
            direction: crate::activity_log_view_model::ActivityDirection::Inbound,
            peer_label: format!("Peer {id}"),
            file_label: format!("File {id}"),
            action: "Started".into(),
            outcome,
            detail: None,
            raw_detail: None,
            bytes: None,
            attempt: 1,
        }
    }

    #[test]
    fn normalize_trims_and_lowercases_unicode() {
        assert_eq!(normalize("  Hello World  "), "hello world");
        assert_eq!(normalize("Grüße"), "grüße");
        assert_eq!(normalize("İSTANBUL"), "i̇stanbul");
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("   "), "");
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(query_matches("", &["anything"]));
        assert!(query_matches("   ", &["anything"]));
        assert!(!query_matches("zzz", &["anything"]));
    }

    #[test]
    fn matching_is_case_insensitive_unicode_substring() {
        assert!(query_matches("REPORT", &["Q3-report.PDF", "peer"]));
        assert!(query_matches("PEER", &["peer-alpha"]));
        assert!(query_matches("straße", &["Hauptstraße.pdf"]));
        assert!(!query_matches("report", &["repost.pdf"]));
    }

    #[test]
    fn long_and_unicode_names_match_fully() {
        let long = "🧾 very long file name with unicode — ünïcödé report v2 (final).pdf";
        assert!(query_matches("unicode", &[long]));
        assert!(query_matches("ÜNÏCÖDÉ", &[long]));
        assert!(query_matches(&long[..20], &[long]));
        assert!(!query_matches("not-there", &[long]));
    }

    #[test]
    fn short_peer_id_is_first_eight_chars() {
        assert_eq!(short_peer_id("abcdef0123456789"), "abcdef01");
        assert_eq!(short_peer_id("短い"), "短い");
    }

    #[test]
    fn shared_by_me_default_sort_is_newest_first() {
        let mut rows = vec![
            shared_row("a", "A.txt", 10, Some(1)),
            shared_row("b", "B.txt", 30, Some(2)),
            shared_row("c", "C.txt", 30, Some(3)),
        ];
        SharedByMeSort::default().apply(&mut rows);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        // Newest first; equal timestamps fall back to the stable id tiebreak.
        assert_eq!(ids, ["b", "c", "a"]);
    }

    #[test]
    fn shared_by_me_name_sort_is_alphabetical_and_reversible() {
        let mut rows = vec![
            shared_row("a", "Zeta.txt", 10, Some(1)),
            shared_row("b", "alpha.txt", 30, Some(2)),
            shared_row("c", "Beta.txt", 20, Some(3)),
        ];
        SharedByMeSort {
            key: SharedByMeSortKey::Name,
            descending: false,
        }
        .apply(&mut rows);
        let names: Vec<&str> = rows.iter().map(|r| r.display_name.as_str()).collect();
        assert_eq!(names, ["alpha.txt", "Beta.txt", "Zeta.txt"]);

        SharedByMeSort {
            key: SharedByMeSortKey::Name,
            descending: true,
        }
        .apply(&mut rows);
        let names: Vec<&str> = rows.iter().map(|r| r.display_name.as_str()).collect();
        assert_eq!(names, ["Zeta.txt", "Beta.txt", "alpha.txt"]);
    }

    #[test]
    fn shared_by_me_size_sort_puts_missing_sizes_last_when_descending() {
        let mut rows = vec![
            shared_row("a", "A", 1, None),
            shared_row("b", "B", 2, Some(5)),
            shared_row("c", "C", 3, Some(10)),
        ];
        SharedByMeSort {
            key: SharedByMeSortKey::Size,
            descending: true,
        }
        .apply(&mut rows);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["c", "b", "a"]);
    }

    #[test]
    fn sort_click_toggles_same_key_and_switches_others() {
        let sort = SharedByMeSort::default();
        assert_eq!(sort.key, SharedByMeSortKey::DateShared);
        assert!(sort.descending);

        let toggled = sort.on_key_clicked(SharedByMeSortKey::DateShared);
        assert!(!toggled.descending);

        let switched = sort.on_key_clicked(SharedByMeSortKey::Name);
        assert_eq!(switched.key, SharedByMeSortKey::Name);
        assert!(!switched.descending); // name defaults ascending

        let size = sort.on_key_clicked(SharedByMeSortKey::Size);
        assert!(size.descending); // size defaults descending
    }

    #[test]
    fn downloaded_default_sort_is_newest_completed_first() {
        let mut items = vec![
            completed(1, "a.txt", 100, 1),
            completed(2, "b.txt", 300, 2),
            completed(3, "c.txt", 300, 3),
        ];
        DownloadedSort::default().apply(&mut items);
        let ids: Vec<i64> = items.iter().map(|i| i.row_id).collect();
        assert_eq!(ids, [2, 3, 1]);
    }

    #[test]
    fn downloaded_name_and_size_sorts_are_deterministic() {
        let mut by_name = vec![
            completed(1, "Z.txt", 1, 1),
            completed(2, "a.txt", 2, 2),
            completed(3, "B.txt", 3, 3),
        ];
        DownloadedSort {
            key: DownloadedSortKey::Name,
            descending: false,
        }
        .apply(&mut by_name);
        let names: Vec<&str> = by_name.iter().map(|i| i.display_name.as_str()).collect();
        assert_eq!(names, ["a.txt", "B.txt", "Z.txt"]);

        let mut by_size = vec![
            completed(1, "a", 1, 5),
            completed(2, "b", 2, 50),
            completed(3, "c", 3, 20),
        ];
        DownloadedSort {
            key: DownloadedSortKey::Size,
            descending: true,
        }
        .apply(&mut by_size);
        let sizes: Vec<u64> = by_size.iter().map(|i| i.size_bytes).collect();
        assert_eq!(sizes, [50, 20, 5]);
    }

    #[test]
    fn downloaded_ref_sort_matches_owned_sort() {
        let items = vec![
            completed(1, "z.txt", 100, 5),
            completed(2, "a.txt", 300, 50),
            completed(3, "m.txt", 300, 20),
        ];
        let mut owned = items.clone();
        DownloadedSort {
            key: DownloadedSortKey::Name,
            descending: false,
        }
        .apply(&mut owned);
        let mut refs: Vec<&CompletedDownloadItem> = items.iter().collect();
        DownloadedSort {
            key: DownloadedSortKey::Name,
            descending: false,
        }
        .apply_ref(&mut refs);
        let owned_ids: Vec<i64> = owned.iter().map(|i| i.row_id).collect();
        let ref_ids: Vec<i64> = refs.iter().map(|i| i.row_id).collect();
        assert_eq!(ref_ids, owned_ids);
        assert_eq!(owned_ids, [2, 3, 1]);
    }

    #[test]
    fn activity_time_sort_is_newest_first_with_tiebreak() {
        let mut rows = vec![
            activity("a", 100, ActivityOutcome::Info),
            activity("b", 300, ActivityOutcome::Success),
            activity("c", 300, ActivityOutcome::Error),
        ];
        ActivitySort::default().apply(&mut rows);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["b", "c", "a"]);
    }

    #[test]
    fn activity_status_sort_orders_severity_then_time() {
        let mut rows = vec![
            activity("a", 500, ActivityOutcome::Info),
            activity("b", 100, ActivityOutcome::Error),
            activity("c", 300, ActivityOutcome::Error),
            activity("d", 200, ActivityOutcome::Success),
            activity("e", 50, ActivityOutcome::Warning),
        ];
        ActivitySort {
            key: ActivitySortKey::Status,
            descending: true,
        }
        .apply(&mut rows);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        // Errors first (newest error first), then warning, success, info.
        assert_eq!(ids, ["c", "b", "e", "d", "a"]);
    }

    #[test]
    fn sort_state_survives_navigation_as_a_plain_value() {
        // The retention contract: sort state is a Copy value stored on the
        // app's screen state (like the active tab and query), so navigating
        // away and back simply re-reads it. This test pins the copy/toggle
        // semantics that make that possible.
        let mut state = DownloadedSort::default();
        state = state.on_key_clicked(DownloadedSortKey::Name);
        let restored = state; // Copy — same as reading it back after navigation
        assert_eq!(restored.key, DownloadedSortKey::Name);
        assert!(!restored.descending);
        assert_eq!(restored, state);
    }

    #[test]
    fn duplicate_display_names_break_ties_by_stable_id() {
        let mut rows = vec![
            shared_row("z-id", "same name", 10, Some(1)),
            shared_row("a-id", "same name", 10, Some(1)),
            shared_row("m-id", "same name", 10, Some(1)),
        ];
        SharedByMeSort {
            key: SharedByMeSortKey::Name,
            descending: false,
        }
        .apply(&mut rows);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["a-id", "m-id", "z-id"]);
    }
}
