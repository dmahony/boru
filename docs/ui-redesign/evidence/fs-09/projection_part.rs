use std::collections::HashMap;

use boru_core::storage::{FileObject, SharedFilePermission, SharedFileRow};


// ── Projection types ────────────────────────────────────────────────────

/// How a recipient currently stands against a shared file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecipientAccess {
    /// Active `read` grant (or friends-fallback when there are no explicit
    /// grants at all — see [`SharedByMeRow::has_explicit_recipients`]).
    Allowed,
    /// A `read` grant whose expiry has passed — access is no longer active.
    Expired,
    /// An explicit `deny` grant — this peer is blocked.
    Denied,
}

/// One recipient shown in the "Shared with" column.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecipientView {
    /// Stable grantee user id (hex public key).
    pub(crate) id: String,
    /// Display label resolved at the application layer.
    pub(crate) label: String,
    /// Current access state derived from the persisted grant.
    pub(crate) access: RecipientAccess,
}

/// A stable, render-ready row for a locally shared file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SharedByMeRow {
    /// Stable row identity — `local:{profile}:{metadata_id}`. Never changes
    /// across refreshes and survives restarts, so list keys and open menus
    /// stay truthful.
    pub(crate) id: String,
    /// Content hash of the underlying file object. Used only to route
    /// actions (reveal, revoke, stop sharing); never displayed in full.
    pub(crate) content_hash: String,
    /// Display filename (no path components).
    pub(crate) display_name: String,
    /// MIME type when the file object is available, else `None`.
    pub(crate) mime_type: Option<String>,
    /// Size in bytes when the file object is available, else `None`
    /// (unknown size must render safely as "—").
    pub(crate) size_bytes: Option<u64>,
    /// When the offer was created (local time is applied at render time).
    pub(crate) shared_on_ms: u64,
    /// Recipient grants for this file. `deny` grants are included so the
    /// access summary is truthful (a blocked peer is still a known actor).
    pub(crate) recipients: Vec<RecipientView>,
    /// Whether the file has any active explicit `read` grants. When `false`
    /// the file is visible to all friends (the product's friends-fallback
    /// semantics) and the column shows "All friends".
    pub(crate) has_explicit_recipients: bool,
    /// Whether the local source file is still known to storage
    /// (`source_path` present). Full paths are never exposed.
    pub(crate) source_available: bool,
    /// Durable download count. The current product has no per-file outbound
    /// download counter (FS-01 finding), so this is always `None` today and
    /// renders as "—"; the column exists for the mockup hierarchy and will
    /// light up once such tracking is persisted.
    pub(crate) downloads: Option<u64>,
}

/// Build the Shared by Me projection from the persisted/shared state.
///
/// `rows` come straight from `Storage::list_shared_files(profile, true)`;
/// `objects` and `permissions` are keyed by content hash. Sorting is
/// deterministic: newest shared first, stable id tiebreak.
pub(crate) fn build_shared_by_me(
    rows: &[SharedFileRow],
    objects: &HashMap<String, FileObject>,
    permissions: &HashMap<String, Vec<SharedFilePermission>>,
    now_ms: u64,
) -> Vec<SharedByMeRow> {
    let mut out: Vec<SharedByMeRow> = rows
        .iter()
        .map(|row| project_row(row, objects, permissions, now_ms))
        .collect();
    out.sort_by(|a, b| {
        b.shared_on_ms
            .cmp(&a.shared_on_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

fn project_row(
    row: &SharedFileRow,
    objects: &HashMap<String, FileObject>,
    permissions: &HashMap<String, Vec<SharedFilePermission>>,
    now_ms: u64,
) -> SharedByMeRow {
    let object = objects.get(&row.content_hash);

    let mut recipients = Vec::new();
    let mut has_explicit_recipients = false;
    let mut explicitly_denied = false;
    if let Some(perms) = permissions.get(&row.content_hash) {
        for permission in perms {
            let active = permission
                .expires_at_ms
                .is_none_or(|expires| expires > now_ms);
            let access = match permission.permission.as_str() {
                "deny" => {
                    explicitly_denied = true;
                    RecipientAccess::Denied
                }
                "read" => {
                    if active {
                        has_explicit_recipients = true;
                        RecipientAccess::Allowed
                    } else {
                        RecipientAccess::Expired
                    }
                }
                _ => continue,
            };
            recipients.push(RecipientView {
                id: permission.grantee_user_id.clone(),
                label: permission.grantee_user_id.clone(),
                access,
            });
        }
    }
    // Deterministic recipient order: allowed first, then expired, then
    // denied, then by id — the same order every frame and every restart.
    recipients.sort_by(|a, b| {
        access_rank(a.access)
            .cmp(&access_rank(b.access))
            .then_with(|| a.id.cmp(&b.id))
    });
    // Never leak a local path: the row carries only a boolean.
    let source_available = object
        .as_ref()
        .is_some_and(|value| value.source_path.is_some());
    // A file with an explicit deny and no read grants is still shared to
    // friends; only explicit reads flip the access model to selected peers.
    let _ = explicitly_denied;

    SharedByMeRow {
        id: format!("local:{}:{}", row.profile_user_id, row.metadata_id),
        content_hash: row.content_hash.clone(),
        display_name: row.display_filename.clone(),
        mime_type: object.as_ref().map(|value| value.mime_type.clone()),
        size_bytes: object.as_ref().map(|value| value.size),
        shared_on_ms: row.created_at_ms,
        recipients,
        has_explicit_recipients,
        source_available,
        downloads: None,
    }
}

fn access_rank(access: RecipientAccess) -> u8 {
    match access {
        RecipientAccess::Allowed => 0,
        RecipientAccess::Expired => 1,
        RecipientAccess::Denied => 2,
    }
}

/// Resolve a grantee's display label at the application layer.
///
/// The projection cannot name peers (it has no friends store), so callers
/// pass labels back in. Labels must never contain local paths.
pub(crate) fn relabel_recipients(
    mut rows: Vec<SharedByMeRow>,
    labels: &HashMap<String, String>,
) -> Vec<SharedByMeRow> {
    for row in &mut rows {
        for recipient in &mut row.recipients {
            if let Some(label) = labels.get(&recipient.id) {
                recipient.label = label.clone();
            }
        }
    }
    rows
}

// ── Formatting helpers ──────────────────────────────────────────────────

/// Format an optional byte count; `None` renders as "—" (unknown size).
pub(crate) fn format_size(size_bytes: Option<u64>) -> String {
    match size_bytes {
        Some(bytes) => crate::dashboard_view_model::format_bytes(bytes),
        None => "—".to_string(),
    }
}

/// Local-time "shared on" label: date plus time in the user's local zone.
///
/// Uses `chrono::Local` like the rest of the GUI's timestamp formatting, so
/// the user's system timezone (Melbourne etc.) is respected automatically.
pub(crate) fn format_shared_on(shared_on_ms: u64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(shared_on_ms as i64).single() {
        Some(dt) => dt.format("%d %b %Y, %H:%M").to_string(),
        None => "—".to_string(),
    }
}

/// Same as [`format_shared_on`] but with an explicit offset — used by tests
/// to get deterministic labels without depending on the host timezone.
pub(crate) fn format_shared_on_with(shared_on_ms: u64, offset_seconds: i32) -> String {
    use chrono::{FixedOffset, TimeZone};
    let offset = match FixedOffset::east_opt(offset_seconds) {
        Some(value) => value,
        None => return "—".to_string(),
    };
    match offset.timestamp_millis_opt(shared_on_ms as i64).single() {
        Some(dt) => dt.format("%d %b %Y, %H:%M").to_string(),
        None => "—".to_string(),
    }
}

/// Short relative age for the row metadata line ("shared 3h ago").
pub(crate) fn relative_shared(shared_on_ms: u64, now_ms: u64) -> String {
    crate::presentation::relative_time_at(shared_on_ms, now_ms, 10)
}

/// Compact kind label for the secondary metadata line. Falls back to "File"
/// when the MIME type is unknown so long/empty values never break layout.
pub(crate) fn kind_label(mime_type: Option<&str>) -> String {
    match mime_type {
        Some(value) if !value.is_empty() => value.to_string(),
        _ => "File".to_string(),
    }
}

/// Truncate a display name for the two-line row; safe for Unicode.
pub(crate) fn truncated_name(name: &str, max_chars: usize) -> String {
    crate::presentation::truncate_with_ellipsis(name, max_chars)
}

// ── View state ──────────────────────────────────────────────────────────

/// Load state for the card body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SharedByMeLoadState {
    /// Storage is being opened / refreshed; render skeleton rows.
    Loading,
    /// Projection is ready — either rows or the empty state.
    Ready,
    /// Storage is unavailable; render a truthful error state.
    Error(String),
}

/// Per-card interactive state (keyed by content hash so row identity stays
/// stable while menus/details/confirmations are open).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SharedByMeUiState {
    /// Row whose action menu is open.
    pub(crate) menu_open: Option<String>,
    /// Row whose details/access panel is open.
    pub(crate) details_open: Option<String>,
    /// Row awaiting "Stop sharing" confirmation.
    pub(crate) confirm_stop: Option<String>,
}

impl SharedByMeUiState {
    pub(crate) fn clear(&mut self) {
        self.menu_open = None;
        self.details_open = None;
        self.confirm_stop = None;
    }

    pub(crate) fn toggle_menu(&mut self, hash: &str) {
        if self.menu_open.as_deref() == Some(hash) {
            self.menu_open = None;
        } else {
            self.menu_open = Some(hash.to_owned());
        }
        // Opening the menu never leaves another panel half-open.
        if self.menu_open.is_some() {
            self.details_open = None;
            self.confirm_stop = None;
        }
    }

    pub(crate) fn open_details(&mut self, hash: &str) {
        self.details_open = Some(hash.to_owned());
        self.menu_open = None;
        self.confirm_stop = None;
    }
}
