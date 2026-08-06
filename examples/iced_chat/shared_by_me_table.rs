//! FS-09 — "Files I'm Sharing" table for the File Sharing dashboard.
//!
//! This module is the Shared by Me tab's primary surface. It binds the
//! persisted [`SharedFileRow`] projection (plus file objects and per-file
//! permission grants) into stable, render-ready rows and renders them as a
//! table card with the design-system hierarchy:
//!
//! - Card header: title, supporting description, green "+ Share Files or
//!   Folder" action, item count.
//! - Column header: Name | Shared with | Size | Shared on | Downloads | (actions)
//! - Row: file/folder icon, two-line name + kind metadata, recipient chip
//!   stack with overflow, local-time "Shared on" date, download count, and a
//!   trailing action menu (details / reveal locally / manage access / stop
//!   sharing).
//!
//! Security/privacy rules enforced here:
//! - Never render a full local path in the table. The projection deliberately
//!   does not carry `source_path`; only a boolean `source_available` flag.
//! - Recipients come from persisted permission grants; expired grants are
//!   shown as expired, deny grants as blocked.
//! - Destructive "Stop sharing" requires an inline confirmation that
//!   truthfully states active-transfer effects before calling the existing
//!   revocation flow (`delete_shared_file`).
//! - The native OS file picker (`AddSharedFile`) remains the only file
//!   selection mechanism — this module never opens an in-app browser.
//!
//! The module is deliberately data-only in its projection half (testable
//! without a database) and widget-only in its view half (no storage access).

use std::collections::HashMap;

use boru_core::storage::{FileObject, SharedFilePermission, SharedFileRow};

use crate::design_tokens;
use crate::fonts::TypeRole;
use crate::icon_system::{Icon, IconSize};

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
    /// Whether the compact "+ Share Files or Folder" menu is open.
    pub(crate) share_menu_open: bool,
    /// Nonblocking registration status shown under the card header while a
    /// selected item is read/hashed/registered. Carries only a display name —
    /// never a full local path.
    pub(crate) sharing_status: Option<String>,
}

impl SharedByMeUiState {
    pub(crate) fn clear(&mut self) {
        self.menu_open = None;
        self.details_open = None;
        self.confirm_stop = None;
        self.share_menu_open = false;
        self.sharing_status = None;
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
            // A row action menu and the header share menu are mutually
            // exclusive popovers.
            self.share_menu_open = false;
        }
    }

    pub(crate) fn open_details(&mut self, hash: &str) {
        self.details_open = Some(hash.to_owned());
        self.menu_open = None;
        self.confirm_stop = None;
    }

    /// Toggle the compact share menu. Opening it closes any row action menu
    /// so the header action is never ambiguous with a row's menu.
    pub(crate) fn toggle_share_menu(&mut self) {
        self.share_menu_open = !self.share_menu_open;
        if self.share_menu_open {
            self.menu_open = None;
            self.details_open = None;
            self.confirm_stop = None;
        }
    }

    pub(crate) fn close_share_menu(&mut self) {
        self.share_menu_open = false;
    }
}

// ── View ────────────────────────────────────────────────────────────────

use iced::widget::{button, container, text, tooltip, Column, Row, Space};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::app::{text_muted_style, AppMessage, BUTTON_PRIMARY_GREEN};

/// Fixed visual widths for the table columns (px). Kept as constants so the
/// column header and every row share identical geometry.
const COL_SHARED_WITH: f32 = 168.0;
const COL_SIZE: f32 = 64.0;
const COL_SHARED_ON: f32 = 122.0;
const COL_DOWNLOADS: f32 = 56.0;
const COL_ACTIONS: f32 = 36.0;

/// Maximum recipient chips shown inline before the "+N more" overflow chip.
const MAX_VISIBLE_CHIPS: usize = 3;
/// Maximum characters of a recipient label shown inside a chip.
const CHIP_LABEL_MAX_CHARS: usize = 14;

/// Build the full "Files I'm Sharing" card.
///
/// `ui` carries the open menu/details/confirmation state owned by the
/// application; `dark_mode` selects the avatar palette. `thumbnails` maps a
/// content hash to an optional image handle — image/video rows render their
/// preview at a uniform size via [`crate::ui_components::file_thumbnail`].
/// The theme and load state are taken by value so the returned element
/// borrows only from `rows`, `ui`, and `thumbnails` (all owned by the app) —
/// the same ownership pattern as the Sharing Summary card.
pub(crate) fn view_shared_by_me_card(
    rows: &[SharedByMeRow],
    ui: &SharedByMeUiState,
    load_state: SharedByMeLoadState,
    theme: Theme,
    dark_mode: bool,
    thumbnails: &HashMap<String, Option<iced::widget::image::Handle>>,
) -> Element<'static, AppMessage> {
    let body: Element<'static, AppMessage> = match &load_state {
        SharedByMeLoadState::Loading => skeleton_body(&theme),
        SharedByMeLoadState::Error(message) => error_body(&theme, message.clone()),
        SharedByMeLoadState::Ready => {
            if rows.is_empty() {
                empty_body(&theme)
            } else {
                table_body(rows, ui, &theme, dark_mode, thumbnails)
            }
        }
    };

    container(
        Column::new()
            .push(card_header(rows.len(), ui, &theme))
            .push(if let Some(status) = &ui.sharing_status {
                container(
                    Row::new()
                        .push(
                            text(status.clone())
                                .size(TypeRole::SupportingText.size_px())
                                .font(TypeRole::SupportingText.font())
                                .style(text_muted_style),
                        )
                        .spacing(design_tokens::SPACE_4)
                        .align_y(Alignment::Center),
                )
                .padding([design_tokens::SPACE_4, design_tokens::SPACE_4])
                .width(Length::Fill)
                .into()
            } else {
                let placeholder: iced::Element<'static, AppMessage> =
                    Space::new().height(0.0).into();
                placeholder
            })
            .push(Space::new().height(design_tokens::SPACE_8))
            .push(body)
            .spacing(0)
            .width(Length::Fill),
    )
    .padding([design_tokens::SPACE_16, design_tokens::SPACE_16])
    .width(Length::Fill)
    .style(|t| design_tokens::card_style(t))
    .into()
}

/// The two items in the compact share menu — exactly "Share Files..." and
/// "Share Folder...". Both return into the existing secure share flow: files
/// reuse `AddSharedFile` (the native OS picker → content-addressed
/// registration); folders reuse the same picker family and the limitation is
/// made explicit by the application layer (the secure catalogue is file-based,
/// so a folder is never silently flattened or faked as a row).
const SHARE_MENU_ITEMS: [(&str, AppMessage); 2] = [
    ("Share Files...", AppMessage::AddSharedFile),
    ("Share Folder...", AppMessage::AddSharedFolder),
];

fn share_menu(theme: &Theme) -> Element<'static, AppMessage> {
    let item = |label: &'static str, message: AppMessage| {
        button(
            Row::new()
                .push(
                    text(label)
                        .size(TypeRole::ButtonLabel.size_px())
                        .font(TypeRole::ButtonLabel.font()),
                )
                .spacing(design_tokens::SPACE_4)
                .align_y(Alignment::Center),
        )
        .on_press(message)
        .padding([design_tokens::SPACE_6, design_tokens::SPACE_10])
        .width(Length::Fill)
        .style(|t, status| {
            let background = match status {
                iced::widget::button::Status::Hovered => design_tokens::surface_hover(t),
                _ => iced::Color::TRANSPARENT,
            };
            iced::widget::button::Style {
                background: Some(Background::Color(background)),
                text_color: design_tokens::text_primary(t),
                border: Border {
                    radius: design_tokens::RADIUS_MD.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
    };

    container(
        Column::new()
            .push(item(SHARE_MENU_ITEMS[0].0, SHARE_MENU_ITEMS[0].1.clone()))
            .push(item(SHARE_MENU_ITEMS[1].0, SHARE_MENU_ITEMS[1].1.clone()))
            .spacing(design_tokens::SPACE_2)
            .width(Length::Fixed(176.0)),
    )
    .padding([design_tokens::SPACE_4, design_tokens::SPACE_4])
    .style(move |t| container::Style {
        background: Some(Background::Color(design_tokens::surface(t))),
        border: Border {
            color: design_tokens::border_muted(t),
            radius: design_tokens::RADIUS_MD.into(),
            width: 1.0,
        },
        ..Default::default()
    })
    .into()
}

fn card_header(
    count: usize,
    ui: &SharedByMeUiState,
    theme: &Theme,
) -> Element<'static, AppMessage> {
    let menu_open = ui.share_menu_open;
    let title_block = Column::new()
        .push(
            text("Files I'm Sharing")
                .size(TypeRole::CardTitle.size_px())
                .font(TypeRole::CardTitle.font())
                .color(design_tokens::text_primary(theme)),
        )
        .push(
            text("Local files and folders you've made available to peers.")
                .size(TypeRole::SupportingText.size_px())
                .font(TypeRole::SupportingText.font())
                .style(text_muted_style),
        )
        .spacing(design_tokens::SPACE_4)
        .width(Length::Fill);

    let share_button = button(
        Row::new()
            .push(
                Icon::Upload
                    .build()
                    .size(IconSize::Xs)
                    .color_fn(|_| Color::WHITE)
                    .build(),
            )
            .push(
                text("Share Files or Folder")
                    .size(TypeRole::ButtonLabel.size_px())
                    .font(TypeRole::ButtonLabel.font()),
            )
            .spacing(design_tokens::SPACE_4)
            .align_y(Alignment::Center),
    )
    .on_press(AppMessage::SharedByMeToggleShareMenu)
    .padding([design_tokens::SPACE_6, design_tokens::SPACE_12])
    .style(BUTTON_PRIMARY_GREEN);

    let mut share_control = Column::new()
        .push(share_button)
        .align_x(Alignment::End)
        .spacing(design_tokens::SPACE_4)
        .width(Length::Shrink);
    if menu_open {
        share_control = share_control.push(share_menu(theme));
    }

    let count_badge = container(
        text(count.to_string())
            .size(TypeRole::Metadata.size_px())
            .font(TypeRole::Metadata.font())
            .color(design_tokens::text_secondary(theme)),
    )
    .padding([2.0, design_tokens::SPACE_8])
    .style(move |t| container::Style {
        background: Some(Background::Color(design_tokens::surface_hover(t))),
        border: Border {
            radius: design_tokens::SPACE_12.into(),
            ..Default::default()
        },
        ..Default::default()
    });

    Row::new()
        .push(title_block)
        .push(count_badge)
        .push(Space::new().width(Length::Fixed(design_tokens::SPACE_8)))
        .push(share_control)
        .align_y(Alignment::Start)
        .spacing(design_tokens::SPACE_8)
        .width(Length::Fill)
        .into()
}

fn column_header(theme: &Theme) -> Element<'static, AppMessage> {
    let cell = |label: &'static str, width: Length| {
        text(label)
            .size(TypeRole::Metadata.size_px())
            .font(TypeRole::Metadata.font())
            .color(design_tokens::text_muted(theme))
            .width(width)
    };
    container(
        Row::new()
            .push(cell("NAME", Length::Fill))
            .push(cell("SHARED WITH", Length::Fixed(COL_SHARED_WITH)))
            .push(cell("SIZE", Length::Fixed(COL_SIZE)))
            .push(cell("SHARED ON", Length::Fixed(COL_SHARED_ON)))
            .push(cell("DOWNLOADS", Length::Fixed(COL_DOWNLOADS)))
            .push(Space::new().width(Length::Fixed(COL_ACTIONS)))
            .spacing(design_tokens::SPACE_8)
            .align_y(Alignment::Center)
            .width(Length::Fill),
    )
    .padding([design_tokens::SPACE_4, design_tokens::SPACE_8])
    .width(Length::Fill)
    .into()
}

fn table_body(
    rows: &[SharedByMeRow],
    ui: &SharedByMeUiState,
    theme: &Theme,
    dark_mode: bool,
    thumbnails: &HashMap<String, Option<iced::widget::image::Handle>>,
) -> Element<'static, AppMessage> {
    let mut children: Vec<Element<'static, AppMessage>> = Vec::with_capacity(rows.len() + 1);
    children.push(column_header(theme));
    for (index, row) in rows.iter().enumerate() {
        let menu_open = ui.menu_open.as_deref() == Some(row.content_hash.as_str());
        let details_open = ui.details_open.as_deref() == Some(row.content_hash.as_str());
        let confirm_stop = ui.confirm_stop.as_deref() == Some(row.content_hash.as_str());
        children.push(
            view_row(
                row,
                menu_open,
                details_open,
                confirm_stop,
                theme,
                dark_mode,
                thumbnails,
            )
            .into(),
        );
        if index < rows.len().saturating_sub(1) {
            children.push(
                container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
                    .width(Length::Fill)
                    .style(move |t| container::Style {
                        background: Some(Background::Color(design_tokens::border_muted(t))),
                        ..Default::default()
                    })
                    .into(),
            );
        }
    }
    let list = Column::with_children(children)
        .spacing(0)
        .width(Length::Fill);
    // The card body scrolls; the footer count is rendered below the list so
    // "Showing X of Y items" stays visible without needing the scrollbar.
    Column::new()
        .push(crate::ui_components::gutter_scrollable(list).width(Length::Fill).height(Length::Shrink))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(footer_count(rows.len(), theme))
        .spacing(0)
        .width(Length::Fill)
        .into()
}

fn footer_count(count: usize, theme: &Theme) -> Element<'static, AppMessage> {
    text(format!(
        "Showing {count} item{}",
        if count == 1 { "" } else { "s" }
    ))
    .size(TypeRole::Metadata.size_px())
    .font(TypeRole::Metadata.font())
    .color(design_tokens::text_muted(theme))
    .into()
}

fn view_row(
    row: &SharedByMeRow,
    menu_open: bool,
    details_open: bool,
    confirm_stop: bool,
    theme: &Theme,
    dark_mode: bool,
    thumbnails: &HashMap<String, Option<iced::widget::image::Handle>>,
) -> Element<'static, AppMessage> {
    let name_cell = name_cell(row, theme, thumbnails);
    let shared_with_cell = shared_with_cell(row, theme, dark_mode);
    let size_cell = text(format_size(row.size_bytes))
        .size(TypeRole::Metadata.size_px())
        .font(TypeRole::Metadata.font())
        .color(design_tokens::text_secondary(theme))
        .width(Length::Fixed(COL_SIZE));
    let shared_on_cell = text(format_shared_on(row.shared_on_ms))
        .size(TypeRole::Metadata.size_px())
        .font(TypeRole::Metadata.font())
        .color(design_tokens::text_secondary(theme))
        .width(Length::Fixed(COL_SHARED_ON));
    let downloads_cell = downloads_cell(row, theme);
    let actions_cell: Element<'static, AppMessage> = if confirm_stop {
        // During inline confirmation the trailing menu is hidden so the
        // user can only Cancel or Confirm.
        Space::new()
            .width(Length::Fixed(COL_ACTIONS))
            .height(Length::Shrink)
            .into()
    } else {
        actions_cell(row, menu_open, theme).into()
    };

    let main_row = Row::new()
        .push(name_cell)
        .push(shared_with_cell)
        .push(size_cell)
        .push(shared_on_cell)
        .push(downloads_cell)
        .push(actions_cell)
        .spacing(design_tokens::SPACE_8)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    let mut column = Column::new().push(main_row).spacing(0).width(Length::Fill);

    if confirm_stop {
        column = column.push(stop_sharing_confirmation(row, theme));
    } else if menu_open {
        column = column.push(action_menu(row, theme));
    }
    if details_open {
        column = column.push(details_panel(row, theme, dark_mode));
    }

    container(column)
        .padding([design_tokens::SPACE_8, design_tokens::SPACE_8])
        .width(Length::Fill)
        .style(move |t| container::Style {
            background: Some(Background::Color(
                if menu_open || details_open || confirm_stop {
                    design_tokens::surface_hover(t)
                } else {
                    design_tokens::surface(t)
                },
            )),
            border: Border {
                radius: design_tokens::RADIUS_MD.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

fn name_cell(
    row: &SharedByMeRow,
    theme: &Theme,
    thumbnails: &HashMap<String, Option<iced::widget::image::Handle>>,
) -> Element<'static, AppMessage> {
    // PAPIRUS-11: every row's icon is the central FileTypeIcon component
    // (same resolver/component as the chat cards, PAPIRUS-10) so the same
    // file shows the same icon everywhere. Image/video rows keep their
    // uniform-size thumbnail preview when a handle is available (generated
    // off the UI thread by the application layer); when the preview is
    // missing (still loading / unsupported / non-media) the resolved
    // Papirus icon answers "what type of file is this?".
    let icon = match row.mime_type.as_deref().unwrap_or("") {
        value if value.starts_with("image/") || value.starts_with("video/") => {
            match thumbnails.get(&row.content_hash).and_then(|h| h.as_ref()) {
                // A real preview exists: keep it (previews preserved).
                Some(handle) => {
                    // PAPIRUS-13: the fallback element (only rendered when a
                    // preview is absent) is the central Papirus file-type
                    // icon, never a Lucide Icon.  The row already prints the
                    // filename and a friendly kind label, so the icon is
                    // decorative (PAPIRUS-15): hidden from assistive
                    // technology, no redundant type tooltip.
                    let fallback = crate::download_progress_view::decorative_file_type_icon_element(
                        &row.display_name,
                        row.mime_type.as_deref(),
                        None,
                        crate::file_type_icon::FileTypeIconSize::List,
                        theme,
                    );
                    crate::ui_components::file_thumbnail(Some(handle), fallback, theme)
                }
                // No preview: central Papirus file-type icon, same as chat.
                None => crate::download_progress_view::decorative_file_type_icon_element(
                    &row.display_name,
                    row.mime_type.as_deref(),
                    None,
                    crate::file_type_icon::FileTypeIconSize::List,
                    theme,
                ),
            }
        }
        _ => crate::download_progress_view::decorative_file_type_icon_element(
            &row.display_name,
            row.mime_type.as_deref(),
            None,
            crate::file_type_icon::FileTypeIconSize::List,
            theme,
        ),
    };
    let full_name = row.display_name.clone();
    let name_text = text(truncated_name(&row.display_name, 44))
        .size(TypeRole::Body.size_px())
        .font(TypeRole::Body.font())
        .color(design_tokens::text_primary(theme));

    let kind = kind_label(row.mime_type.as_deref());
    let meta_line = text(format!(
        "{} · {} · shared {}",
        kind,
        format_size(row.size_bytes),
        relative_shared(row.shared_on_ms, now_ms_u64())
    ))
    .size(TypeRole::Metadata.size_px())
    .font(TypeRole::Metadata.font())
    .color(design_tokens::text_muted(theme));

    let name_block = Column::new()
        .push(name_text)
        .push(meta_line)
        .spacing(design_tokens::SPACE_2)
        .width(Length::Fill)
        .align_x(Alignment::Start);

    let with_tooltip: Element<'static, AppMessage> = if row.display_name.chars().count() > 44 {
        tooltip::Tooltip::new(
            name_block,
            text(full_name).size(TypeRole::Metadata.size_px()),
            tooltip::Position::Bottom,
        )
        .into()
    } else {
        name_block.into()
    };

    Row::new()
        .push(icon)
        .push(Space::new().width(Length::Fixed(design_tokens::SPACE_8)))
        .push(with_tooltip)
        .spacing(0)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

fn shared_with_cell(
    row: &SharedByMeRow,
    theme: &Theme,
    dark_mode: bool,
) -> Element<'static, AppMessage> {
    let chips: Vec<Element<'static, AppMessage>> = if row.has_explicit_recipients {
        row.recipients
            .iter()
            .filter(|recipient| recipient.access == RecipientAccess::Allowed)
            .take(MAX_VISIBLE_CHIPS)
            .map(|recipient| recipient_chip(recipient, theme, dark_mode))
            .collect()
    } else {
        vec![all_friends_chip(theme)]
    };

    let mut inner = Row::new()
        .spacing(design_tokens::SPACE_4)
        .align_y(Alignment::Center);

    let mut shown = 0;
    for chip in chips {
        if shown >= MAX_VISIBLE_CHIPS {
            break;
        }
        inner = inner.push(chip);
        shown += 1;
    }
    let active_count = row
        .recipients
        .iter()
        .filter(|recipient| recipient.access == RecipientAccess::Allowed)
        .count();
    if active_count > MAX_VISIBLE_CHIPS {
        inner = inner.push(
            text(format!("+{}", active_count - MAX_VISIBLE_CHIPS))
                .size(TypeRole::Metadata.size_px())
                .font(TypeRole::Metadata.font())
                .color(design_tokens::text_muted(theme)),
        );
    }
    let expired_count = row
        .recipients
        .iter()
        .filter(|recipient| recipient.access == RecipientAccess::Expired)
        .count();
    if expired_count > 0 {
        inner = inner.push(
            text(format!("{} expired", expired_count))
                .size(TypeRole::Metadata.size_px())
                .font(TypeRole::Metadata.font())
                .color(design_tokens::text_muted(theme)),
        );
    }

    container(inner)
        .width(Length::Fixed(COL_SHARED_WITH))
        .align_x(Alignment::Start)
        .into()
}

fn recipient_chip(
    recipient: &RecipientView,
    theme: &Theme,
    dark_mode: bool,
) -> Element<'static, AppMessage> {
    let initial = crate::presentation::initials(&recipient.label)
        .chars()
        .take(1)
        .collect::<String>();
    let color = crate::presentation::initials_color(&recipient.label, dark_mode);
    let initial_label = if initial.is_empty() {
        "?".to_string()
    } else {
        initial
    };
    let dot = container(
        text(initial_label)
            .size(9.0)
            .color(Color::WHITE),
    )
    .width(Length::Fixed(16.0))
    .height(Length::Fixed(16.0))
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .style(move |_t| container::Style {
        background: Some(Background::Color(color)),
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    });

    let label = text(truncated_name(&recipient.label, CHIP_LABEL_MAX_CHARS))
        .size(TypeRole::Metadata.size_px())
        .font(TypeRole::Metadata.font())
        .color(design_tokens::text_secondary(theme));

    container(
        Row::new()
            .push(dot)
            .push(Space::new().width(Length::Fixed(design_tokens::SPACE_4)))
            .push(label)
            .spacing(0)
            .align_y(Alignment::Center),
    )
    .padding([2.0, design_tokens::SPACE_6])
    .style(move |t| container::Style {
        background: Some(Background::Color(design_tokens::surface_hover(t))),
        border: Border {
            color: design_tokens::border_muted(t),
            width: 1.0,
            radius: design_tokens::SPACE_12.into(),
        },
        ..Default::default()
    })
    .into()
}

fn all_friends_chip(theme: &Theme) -> Element<'static, AppMessage> {
    container(
        text("All friends")
            .size(TypeRole::Metadata.size_px())
            .font(TypeRole::Metadata.font())
            .color(design_tokens::text_secondary(theme)),
    )
    .padding([2.0, design_tokens::SPACE_6])
    .style(move |t| container::Style {
        background: Some(Background::Color(design_tokens::surface_hover(t))),
        border: Border {
            color: design_tokens::border_muted(t),
            width: 1.0,
            radius: design_tokens::SPACE_12.into(),
        },
        ..Default::default()
    })
    .into()
}

fn downloads_cell(row: &SharedByMeRow, theme: &Theme) -> Element<'static, AppMessage> {
    let value = match row.downloads {
        Some(count) => format!("↓{count}"),
        // No durable per-file counter exists yet; a muted dash is truthful
        // and keeps the column aligned with the mockup hierarchy.
        None => "—".to_string(),
    };
    text(value)
        .size(TypeRole::Metadata.size_px())
        .font(TypeRole::Metadata.font())
        .color(design_tokens::text_muted(theme))
        .width(Length::Fixed(COL_DOWNLOADS))
        .into()
}

fn actions_cell(
    row: &SharedByMeRow,
    menu_open: bool,
    theme: &Theme,
) -> Element<'static, AppMessage> {
    let icon = Icon::MoreVertical
        .build()
        .size(IconSize::Sm)
        .color_fn(if menu_open {
            design_tokens::primary
        } else {
            design_tokens::text_secondary
        })
        .build();
    let button = button(icon)
        .on_press(AppMessage::SharedByMeMenuToggle(row.content_hash.clone()))
        .padding([design_tokens::SPACE_2, design_tokens::SPACE_4])
        .style(move |t, status| button::Style {
            background: match status {
                button::Status::Hovered => Some(Background::Color(design_tokens::surface_hover(t))),
                _ => None,
            },
            text_color: design_tokens::text_secondary(t),
            border: Border {
                radius: design_tokens::RADIUS_SM.into(),
                ..Default::default()
            },
            ..Default::default()
        });
    container(button)
        .width(Length::Fixed(COL_ACTIONS))
        .align_x(Alignment::Center)
        .into()
}

fn action_menu(row: &SharedByMeRow, theme: &Theme) -> Element<'static, AppMessage> {
    let hash = row.content_hash.clone();
    let menu_item = |label: &'static str, message: AppMessage| {
        button(
            text(label)
                .size(TypeRole::ButtonLabel.size_px())
                .font(TypeRole::ButtonLabel.font()),
        )
        .on_press(message)
        .padding([design_tokens::SPACE_4, design_tokens::SPACE_8])
        .style(move |t, status| button::Style {
            background: match status {
                button::Status::Hovered => {
                    Some(Background::Color(design_tokens::surface_hover(t)))
                }
                _ => None,
            },
            text_color: design_tokens::text_primary(t),
                border: Border {
                    radius: design_tokens::RADIUS_SM.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
    };

    let mut items = Column::new()
        .spacing(design_tokens::SPACE_2)
        .width(Length::Fill);
    items = items.push(menu_item(
        "Details",
        AppMessage::SharedByMeDetails(hash.clone()),
    ));
    if row.source_available {
        items = items.push(menu_item(
            "Reveal locally",
            AppMessage::SharedByMeReveal(hash.clone()),
        ));
    }
    items = items.push(menu_item(
        "Manage access",
        AppMessage::SharedByMeDetails(hash.clone()),
    ));
    items = items.push(menu_item(
        "Stop sharing",
        AppMessage::SharedByMeConfirmStopSharing(hash.clone()),
    ));

    container(items)
        .padding([design_tokens::SPACE_4, 0.0])
        .width(Length::Fill)
        .style(move |t| container::Style {
            background: Some(Background::Color(design_tokens::surface_hover(t))),
            border: Border {
                color: design_tokens::border_muted(t),
                width: 1.0,
                radius: design_tokens::RADIUS_MD.into(),
            },
            ..Default::default()
        })
        .into()
}

fn stop_sharing_confirmation(
    row: &SharedByMeRow,
    theme: &Theme,
) -> Element<'static, AppMessage> {
    let hash = row.content_hash.clone();
    let prompt_block = Column::new()
        .push(
            text("Stop sharing this item?")
                .size(TypeRole::BodyEmphasised.size_px())
                .font(TypeRole::BodyEmphasised.font())
                .color(design_tokens::text_primary(theme)),
        )
        .push(
            text(
                "Peers with active downloads may lose access mid-transfer. \
                 The file is removed from your shared list; local copies are not deleted.",
            )
            .size(TypeRole::SupportingText.size_px())
            .font(TypeRole::SupportingText.font())
            .color(design_tokens::text_secondary(theme)),
        )
        .spacing(design_tokens::SPACE_2)
        .width(Length::Fill)
        .align_x(Alignment::Start);

    let cancel = button(
        text("Cancel")
            .size(TypeRole::ButtonLabel.size_px())
            .font(TypeRole::ButtonLabel.font()),
    )
    .on_press(AppMessage::SharedByMeCancelStopSharing)
        .padding([design_tokens::SPACE_4, design_tokens::SPACE_10])
        .style(|t, status| button::Style {
            background: match status {
                button::Status::Hovered => Some(Background::Color(design_tokens::surface_hover(t))),
                _ => None,
            },
            text_color: design_tokens::text_secondary(t),
            border: Border {
                color: design_tokens::border_muted(t),
                width: 1.0,
                radius: design_tokens::RADIUS_SM.into(),
            },
            ..Default::default()
        });
    let confirm = button(
        text("Stop sharing")
            .size(TypeRole::ButtonLabel.size_px())
            .font(TypeRole::ButtonLabel.font()),
    )
    .on_press(AppMessage::SharedByMeConfirmStopSharing(hash))
        .padding([design_tokens::SPACE_4, design_tokens::SPACE_10])
        .style(|t, _status| button::Style {
            background: Some(Background::Color(design_tokens::destructive(t))),
            text_color: Color::WHITE,
            border: Border {
                radius: design_tokens::RADIUS_SM.into(),
                ..Default::default()
            },
            ..Default::default()
        });

    container(
        Row::new()
            .push(prompt_block)
            .push(Space::new().width(Length::Fill))
            .push(cancel)
            .push(Space::new().width(Length::Fixed(design_tokens::SPACE_8)))
            .push(confirm)
            .align_y(Alignment::Center)
            .spacing(0)
            .width(Length::Fill),
    )
    .padding([design_tokens::SPACE_8, design_tokens::SPACE_4])
    .width(Length::Fill)
    .style(|t| {
        let soft_bg = design_tokens::destructive_soft(t);
        let destructive = design_tokens::color_danger(t);
        container::Style {
            background: Some(Background::Color(soft_bg)),
            border: Border {
                color: destructive,
                width: 1.0,
                radius: design_tokens::RADIUS_MD.into(),
            },
            ..Default::default()
        }
    })
    .into()
}

fn details_panel(
    row: &SharedByMeRow,
    theme: &Theme,
    dark_mode: bool,
) -> Element<'static, AppMessage> {
    let hash = row.content_hash.clone();
    let detail_line = |label: &'static str, value: String| {
        Row::new()
            .push(
                text(label)
                    .size(TypeRole::Metadata.size_px())
                    .font(TypeRole::Metadata.font())
                    .color(design_tokens::text_muted(theme))
                    .width(Length::Fixed(96.0)),
            )
            .push(
                text(value)
                    .size(TypeRole::Metadata.size_px())
                    .font(TypeRole::Metadata.font())
                    .color(design_tokens::text_secondary(theme)),
            )
            .spacing(design_tokens::SPACE_8)
            .align_y(Alignment::Center)
            .width(Length::Fill)
    };

    let short_hash = if row.content_hash.chars().count() > 12 {
        let mut prefix = row.content_hash.chars().take(12).collect::<String>();
        prefix.push('…');
        prefix
    } else {
        row.content_hash.clone()
    };

    let mut info = Column::new()
        .spacing(design_tokens::SPACE_2)
        .width(Length::Fill);
    info = info.push(detail_line("Name", row.display_name.clone()));
    info = info.push(detail_line("Kind", kind_label(row.mime_type.as_deref())));
    info = info.push(detail_line("Size", format_size(row.size_bytes)));
    info = info.push(detail_line("Shared on", format_shared_on(row.shared_on_ms)));
    info = info.push(detail_line("Content ID", short_hash));
    info = info.push(detail_line(
        "Source",
        if row.source_available {
            "Available locally".to_string()
        } else {
            "Unavailable — source file missing".to_string()
        },
    ));

    // Access summary: list every recipient with its state and a revoke
    // action for active read grants (backed by `revoke_permission`).
    let mut access = Column::new()
        .spacing(design_tokens::SPACE_4)
        .width(Length::Fill);
    access = access.push(
        text("Access")
            .size(TypeRole::Metadata.size_px())
            .font(TypeRole::Metadata.font())
            .color(design_tokens::text_muted(theme)),
    );
    if !row.has_explicit_recipients && row.recipients.is_empty() {
        access = access.push(
            text("Visible to all friends (no explicit grants).")
                .size(TypeRole::SupportingText.size_px())
                .font(TypeRole::SupportingText.font())
                .color(design_tokens::text_secondary(theme)),
        );
    } else {
        for recipient in &row.recipients {
            let (state_label, state_color) = match recipient.access {
                RecipientAccess::Allowed => ("Can access", design_tokens::color_success(theme)),
                RecipientAccess::Expired => ("Expired", design_tokens::text_muted(theme)),
                RecipientAccess::Denied => ("Blocked", design_tokens::destructive(theme)),
            };
            // Hoisted out of the style closure below: the closure must only
            // capture the Copy `Color`, never a borrow of `recipient`, so the
            // whole details panel stays `'static` for the lazy card builder.
            let initial_color = crate::presentation::initials_color(&recipient.label, dark_mode);
            let mut row_builder = Row::new()
                .push(
                    container(
                        text(
                            crate::presentation::initials(&recipient.label)
                                .chars()
                                .take(1)
                                .collect::<String>(),
                        )
                        .size(9.0)
                        .color(Color::WHITE),
                    )
                    .width(Length::Fixed(16.0))
                    .height(Length::Fixed(16.0))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .style(move |_t| container::Style {
                        background: Some(Background::Color(initial_color)),
                        border: Border {
                            radius: 8.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                )
                .push(Space::new().width(Length::Fixed(design_tokens::SPACE_6)))
                .push(
                    text(truncated_name(&recipient.label, 28))
                        .size(TypeRole::Metadata.size_px())
                        .font(TypeRole::Metadata.font())
                        .color(design_tokens::text_primary(theme))
                        .width(Length::Fill),
                )
                .push(
                    text(state_label)
                        .size(TypeRole::Metadata.size_px())
                        .font(TypeRole::Metadata.font())
                        .color(state_color),
                );
            if recipient.access == RecipientAccess::Allowed {
                row_builder = row_builder
                    .push(Space::new().width(Length::Fixed(design_tokens::SPACE_8)))
                    .push(
                        button(
                            text("Revoke")
                                .size(TypeRole::ButtonLabel.size_px())
                                .font(TypeRole::ButtonLabel.font()),
                        )
                        .on_press(AppMessage::SharedByMeRevokeAccess(
                                hash.clone(),
                                recipient.id.clone(),
                            ))
                            .padding([design_tokens::SPACE_2, design_tokens::SPACE_8])
                            .style(|t, status| button::Style {
                                background: match status {
                                    button::Status::Hovered => {
                                        Some(Background::Color(design_tokens::surface_hover(t)))
                                    }
                                    _ => None,
                                },
                                text_color: design_tokens::text_secondary(t),
                                border: Border {
                                    color: design_tokens::border_muted(t),
                                    width: 1.0,
                                    radius: design_tokens::RADIUS_SM.into(),
                                },
                                ..Default::default()
                            }),
                    );
            }
            access = access.push(container(row_builder).width(Length::Fill));
        }
    }

    let close = button(
        text("Close")
            .size(TypeRole::ButtonLabel.size_px())
            .font(TypeRole::ButtonLabel.font()),
    )
    .on_press(AppMessage::SharedByMeCloseDetails)
        .padding([design_tokens::SPACE_4, design_tokens::SPACE_10])
        .style(|t, status| button::Style {
            background: match status {
                button::Status::Hovered => Some(Background::Color(design_tokens::surface_hover(t))),
                _ => None,
            },
            text_color: design_tokens::text_secondary(t),
            border: Border {
                color: design_tokens::border_muted(t),
                width: 1.0,
                radius: design_tokens::RADIUS_SM.into(),
            },
            ..Default::default()
        });

    container(
        Column::new()
            .push(
                Row::new()
                    .push(
                        text("Details")
                            .size(TypeRole::BodyEmphasised.size_px())
                            .font(TypeRole::BodyEmphasised.font())
                            .color(design_tokens::text_primary(theme)),
                    )
                    .push(Space::new().width(Length::Fill))
                    .push(close)
                    .align_y(Alignment::Center)
                    .width(Length::Fill),
            )
            .push(Space::new().height(Length::Fixed(design_tokens::SPACE_6)))
            .push(info)
            .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
            .push(access)
            .spacing(0)
            .width(Length::Fill),
    )
    .padding([design_tokens::SPACE_8, design_tokens::SPACE_4])
    .width(Length::Fill)
    .style(move |t| container::Style {
        background: Some(Background::Color(design_tokens::surface(t))),
        border: Border {
            color: design_tokens::border_muted(t),
            width: 1.0,
            radius: design_tokens::RADIUS_MD.into(),
        },
        ..Default::default()
    })
    .into()
}

// ── Empty / loading / error bodies ──────────────────────────────────────

fn empty_body(theme: &Theme) -> Element<'static, AppMessage> {
    Column::new()
        .push(
            Icon::Upload
                .build()
                .size(IconSize::Xl)
                .color_fn(design_tokens::text_muted)
                .build(),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_16)))
        .push(
            text("You haven't shared any files yet.")
                .size(TypeRole::Body.size_px())
                .font(TypeRole::Body.font())
                .color(design_tokens::text_secondary(theme)),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_4)))
        .push(
            text("Use the Share button in chat or your profile to make files available.")
                .size(TypeRole::SupportingText.size_px())
                .font(TypeRole::SupportingText.font())
                .color(design_tokens::text_muted(theme)),
        )
        .spacing(0)
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .padding([design_tokens::SPACE_32, 0.0])
        .into()
}

fn skeleton_body(theme: &Theme) -> Element<'static, AppMessage> {
    let bar = |width: f32| {
        container(
            Space::new()
                .width(Length::Fixed(width))
                .height(Length::Fixed(10.0)),
        )
        .style(move |t| container::Style {
            background: Some(Background::Color(design_tokens::surface_hover(t))),
            border: Border {
                radius: design_tokens::RADIUS_SM.into(),
                ..Default::default()
            },
            ..Default::default()
        })
    };
    let row = |bar_widths: &[f32]| {
        let mut r = Row::new()
            .spacing(design_tokens::SPACE_8)
            .align_y(Alignment::Center);
        for width in bar_widths {
            r = r.push(bar(*width));
        }
        r = r.push(Space::new().width(Length::Fill));
        container(r)
            .padding([design_tokens::SPACE_8, design_tokens::SPACE_8])
            .width(Length::Fill)
    };
    Column::new()
        .push(row(&[24.0, 120.0]))
        .push(row(&[24.0, 96.0]))
        .push(row(&[24.0, 140.0]))
        .push(row(&[24.0, 80.0]))
        .spacing(design_tokens::SPACE_4)
        .width(Length::Fill)
        .into()
}

fn error_body(theme: &Theme, message: String) -> Element<'static, AppMessage> {
    Column::new()
        .push(
            Icon::AlertTriangle
                .build()
                .size(IconSize::Xl)
                .color_fn(design_tokens::color_danger)
                .build(),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_16)))
        .push(
            text("File sharing storage is unavailable")
                .size(TypeRole::Body.size_px())
                .font(TypeRole::Body.font())
                .color(design_tokens::text_primary(theme)),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_4)))
        .push(
            text(message)
                .size(TypeRole::SupportingText.size_px())
                .font(TypeRole::SupportingText.font())
                .color(design_tokens::text_muted(theme)),
        )
        .spacing(0)
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .padding([design_tokens::SPACE_32, 0.0])
        .into()
}

fn now_ms_u64() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use boru_core::storage::SharedFileRow;

    fn local_row(hash: &str, name: &str, created_at_ms: u64) -> SharedFileRow {
        SharedFileRow {
            content_hash: hash.into(),
            profile_user_id: "local".into(),
            metadata_id: format!("meta-{hash}"),
            display_filename: name.into(),
            description: None,
            offered: true,
            created_at_ms,
            updated_at_ms: created_at_ms,
            version: 1,
        }
    }

    fn object(hash: &str, name: &str, size: u64, source: Option<&str>) -> FileObject {
        FileObject {
            content_hash: hash.into(),
            size,
            mime_type: "application/pdf".into(),
            filename: name.into(),
            created_at_ms: 1,
            data: None,
            source_path: source.map(str::to_owned),
        }
    }

    fn permission(
        hash: &str,
        grantee: &str,
        permission: &str,
        expires_at_ms: Option<u64>,
    ) -> SharedFilePermission {
        SharedFilePermission {
            content_hash: hash.into(),
            grantor_user_id: "local".into(),
            grantee_user_id: grantee.into(),
            permission: permission.into(),
            created_at_ms: 1,
            expires_at_ms,
        }
    }

    #[test]
    fn projection_is_newest_first_with_stable_id_tiebreak() {
        let rows = vec![
            local_row("a", "old.txt", 100),
            local_row("b", "new.txt", 200),
            local_row("c", "same.txt", 200),
        ];
        let out = build_shared_by_me(&rows, &HashMap::new(), &HashMap::new(), 1_000);
        assert_eq!(out[0].id, "local:local:meta-b");
        assert_eq!(out[1].id, "local:local:meta-c");
        assert_eq!(out[2].id, "local:local:meta-a");
        assert_eq!(
            out.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
            out.iter().map(|r| r.id.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn projection_never_contains_local_path() {
        let mut objects = HashMap::new();
        objects.insert(
            "a".into(),
            object("a", "secret.pdf", 42, Some("/home/u/secret.pdf")),
        );
        let rows = vec![local_row("a", "secret.pdf", 100)];
        let out = build_shared_by_me(&rows, &objects, &HashMap::new(), 1_000);
        assert!(out[0].source_available);
        assert!(!format!("{:?}", out[0]).contains("/home/u"));
        assert!(!format!("{:?}", out[0]).contains("source_path"));
        // The bare display filename is expected; a *path* must never leak.
        assert_eq!(out[0].display_name, "secret.pdf");
        assert!(!out[0].display_name.contains('/'));
    }

    #[test]
    fn missing_source_and_unknown_size_render_as_safe_missing_values() {
        let rows = vec![local_row("a", "gone.pdf", 100)];
        let out = build_shared_by_me(&rows, &HashMap::new(), &HashMap::new(), 1_000);
        assert!(!out[0].source_available);
        assert_eq!(out[0].size_bytes, None);
        assert_eq!(format_size(out[0].size_bytes), "—");
        assert_eq!(kind_label(out[0].mime_type.as_deref()), "File");
    }

    #[test]
    fn recipients_are_classified_allowed_expired_denied() {
        let mut perms = HashMap::new();
        perms.insert(
            "a".into(),
            vec![
                permission("a", "peer-allowed", "read", None),
                permission("a", "peer-expired", "read", Some(500)),
                permission("a", "peer-denied", "deny", None),
            ],
        );
        let rows = vec![local_row("a", "a.pdf", 100)];
        let out = build_shared_by_me(&rows, &HashMap::new(), &perms, 1_000);
        assert!(out[0].has_explicit_recipients);
        assert_eq!(out[0].recipients.len(), 3);
        assert_eq!(out[0].recipients[0].access, RecipientAccess::Allowed);
        assert_eq!(out[0].recipients[1].access, RecipientAccess::Expired);
        assert_eq!(out[0].recipients[2].access, RecipientAccess::Denied);
    }

    #[test]
    fn zero_recipients_means_friends_fallback() {
        let rows = vec![local_row("a", "a.pdf", 100)];
        let out = build_shared_by_me(&rows, &HashMap::new(), &HashMap::new(), 1_000);
        assert!(!out[0].has_explicit_recipients);
        assert!(out[0].recipients.is_empty());
    }

    #[test]
    fn downloads_is_untracked_until_durable_counter_exists() {
        let rows = vec![local_row("a", "a.pdf", 100)];
        let out = build_shared_by_me(&rows, &HashMap::new(), &HashMap::new(), 1_000);
        assert_eq!(out[0].downloads, None);
    }

    #[test]
    fn relabel_replaces_grantee_ids_with_friendly_names() {
        let mut perms = HashMap::new();
        perms.insert("a".into(), vec![permission("a", "peer-x", "read", None)]);
        let rows = vec![local_row("a", "a.pdf", 100)];
        let mut out = build_shared_by_me(&rows, &HashMap::new(), &perms, 1_000);
        assert_eq!(out[0].recipients[0].label, "peer-x");
        let mut labels = HashMap::new();
        labels.insert("peer-x".into(), "Alice".into());
        out = relabel_recipients(out, &labels);
        assert_eq!(out[0].recipients[0].label, "Alice");
    }

    #[test]
    fn shared_on_formatting_uses_local_offset() {
        // 2026-08-04 09:12:00 UTC.
        let ms: u64 = 1_785_834_720_000;
        let label = format_shared_on_with(ms, 10 * 3600); // UTC+10 (Melbourne summer)
        assert!(label.contains("04 Aug 2026"), "got {label}");
        assert!(label.contains("19:12"), "got {label}");
        let utc = format_shared_on_with(ms, 0);
        assert!(utc.contains("09:12"), "got {utc}");
    }

    #[test]
    fn truncation_is_unicode_safe() {
        let name = "日本語のとても長いファイル名ファイル名ファイル名.pdf";
        let truncated = truncated_name(name, 12);
        assert!(truncated.chars().count() <= 12);
        assert!(truncated.ends_with('…'));
        assert!(truncated_name("short.txt", 40).ends_with("short.txt"));
    }

    #[test]
    fn card_builds_without_panicking_for_all_states() {
        let theme = Theme::Light;
        let ui = SharedByMeUiState::default();
        let thumbnails: HashMap<String, Option<iced::widget::image::Handle>> = HashMap::new();

        let empty: Vec<SharedByMeRow> = vec![];
        let _ = view_shared_by_me_card(
            &empty,
            &ui,
            SharedByMeLoadState::Loading,
            theme.clone(),
            false,
            &thumbnails,
        );
        let _ = view_shared_by_me_card(
            &empty,
            &ui,
            SharedByMeLoadState::Error("storage unavailable".into()),
            theme.clone(),
            false,
            &thumbnails,
        );
        let _ = view_shared_by_me_card(
            &empty,
            &ui,
            SharedByMeLoadState::Ready,
            theme.clone(),
            false,
            &thumbnails,
        );

        let mut rows = vec![
            SharedByMeRow {
                id: "local:local:meta-a".into(),
                content_hash: "aaaa".repeat(16),
                display_name:
                    "report-with-a-very-long-name-that-will-definitely-exceed-the-limit.pdf".into(),
                mime_type: Some("application/pdf".into()),
                size_bytes: Some(2 * 1024 * 1024),
                shared_on_ms: 1_784_814_720_000,
                recipients: (0..8)
                    .map(|i| RecipientView {
                        id: format!("peer-{i}"),
                        label: format!("Friend Number {i}"),
                        access: RecipientAccess::Allowed,
                    })
                    .collect(),
                has_explicit_recipients: true,
                source_available: true,
                downloads: None,
            },
            SharedByMeRow {
                id: "local:local:meta-b".into(),
                content_hash: "bbbb".repeat(16),
                display_name: "notes.md".into(),
                mime_type: None,
                size_bytes: None,
                shared_on_ms: 1_784_800_000_000,
                recipients: vec![],
                has_explicit_recipients: false,
                source_available: false,
                downloads: None,
            },
        ];
        let _ = view_shared_by_me_card(
            &rows,
            &ui,
            SharedByMeLoadState::Ready,
            theme.clone(),
            false,
            &thumbnails,
        );

        let mut ui_open = SharedByMeUiState::default();
        ui_open.toggle_menu(&rows[0].content_hash);
        let _ = view_shared_by_me_card(
            &rows,
            &ui_open,
            SharedByMeLoadState::Ready,
            theme.clone(),
            true,
            &thumbnails,
        );

        ui_open.open_details(&rows[0].content_hash);
        let _ = view_shared_by_me_card(
            &rows,
            &ui_open,
            SharedByMeLoadState::Ready,
            theme.clone(),
            true,
            &thumbnails,
        );

        let mut ui_confirm = SharedByMeUiState::default();
        ui_confirm.confirm_stop = Some(rows[0].content_hash.clone());
        let _ = view_shared_by_me_card(
            &rows,
            &ui_confirm,
            SharedByMeLoadState::Ready,
            theme.clone(),
            true,
            &thumbnails,
        );
        rows.clear();
    }

    #[test]
    fn ui_state_cycles_are_deterministic() {
        let mut ui = SharedByMeUiState::default();
        ui.toggle_menu("a");
        assert_eq!(ui.menu_open.as_deref(), Some("a"));
        ui.toggle_menu("a");
        assert_eq!(ui.menu_open, None);
        ui.toggle_menu("a");
        ui.open_details("a");
        assert_eq!(ui.menu_open, None);
        assert_eq!(ui.details_open.as_deref(), Some("a"));
        ui.clear();
        assert_eq!(ui, SharedByMeUiState::default());
    }

    #[test]
    fn share_menu_toggle_is_mutually_exclusive_with_row_menus() {
        let mut ui = SharedByMeUiState::default();
        ui.toggle_menu("row-a");
        ui.toggle_share_menu();
        // Opening the header share menu closes row menus.
        assert!(ui.share_menu_open);
        assert_eq!(ui.menu_open, None);
        ui.toggle_share_menu();
        assert!(!ui.share_menu_open);
        // Toggling a row menu while the share menu is open closes the share menu.
        ui.toggle_share_menu();
        ui.toggle_menu("row-b");
        assert!(!ui.share_menu_open);
        assert_eq!(ui.menu_open.as_deref(), Some("row-b"));
        ui.clear();
        assert!(!ui.share_menu_open);
    }

    #[test]
    fn share_menu_items_are_exactly_files_and_folder() {
        let labels: Vec<&str> = SHARE_MENU_ITEMS.iter().map(|(label, _)| *label).collect();
        assert_eq!(labels, vec!["Share Files...", "Share Folder..."]);
        assert!(
            matches!(SHARE_MENU_ITEMS[0].1, AppMessage::AddSharedFile),
            "files must reuse the existing secure share entry point"
        );
        assert!(
            matches!(SHARE_MENU_ITEMS[1].1, AppMessage::AddSharedFolder),
            "folders must route to the native folder picker"
        );
    }

    #[test]
    fn sharing_status_is_cleared_with_the_rest_of_ui_state() {
        let mut ui = SharedByMeUiState::default();
        ui.sharing_status = Some("Registering report.pdf…".into());
        ui.share_menu_open = true;
        ui.clear();
        assert_eq!(ui.sharing_status, None);
        assert!(!ui.share_menu_open);
        assert_eq!(ui, SharedByMeUiState::default());
    }
}
