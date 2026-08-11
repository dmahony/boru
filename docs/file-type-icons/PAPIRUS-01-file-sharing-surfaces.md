# PAPIRUS-01 — Existing Boru File-Sharing Surfaces: File-Type Icon Audit

Task: `t_50f26f47` — Audit every Boru surface that displays a file or folder.
Source spec: `Boru_Papirus_icons.txt` Task 1 (attachment of `t_9d01cfec`).
Scope: analysis only. No production code was modified by this task.
Baseline: `cargo build --bin boru --features gui,video-playback,terminal` — see "Baseline evidence" at the end.

---

## 0. TL;DR

Boru renders **no real file-type icons anywhere**. Every file/folder visual is one of:

- the Lucide `files.svg` glyph (`Icon::Files` / `ICON_FILES`), used for **all** non-media rows in every dashboard table and for every chat download card;
- `Icon::Image` (image rows), `Icon::Play` (video/audio rows), `Icon::Upload` (share buttons / empty states), `Icon::Folder` (sidebar nav only);
- **emoji** in exactly two places: notification body previews (`app.rs:20571-20572`) and the image-unavailable placeholder (`app.rs:28940`);
- **generic text glyphs**: `"▶"` play glyph (`download_progress_view.rs:514`), `"VIDEO"` placeholder (`download_progress_view.rs:489`), `"✓"` status marks;
- **no icon at all** for non-media rows in "Shared with Me" (`app.rs:31897-31901` renders an empty `Space`).

There is no central file-type resolver. Five independent resolver/map implementations exist (see §5). MIME type is available on most dashboard row models but **not** on the chat `DownloadAttachment` nor on the transfer projections (`IncomingTransferRow`, `PeerDownload`, `RecentActivityRow`, `ActivityLogRow`). **Folder state does not exist in any UI model** — Boru cannot share folders today (`app.rs:19350-19372`).

Boru supports light + dark themes via `dark_mode` + `Theme::Dark/Light` (`design_tokens.rs:328-330`, `app.rs:21815-21828`). All current icons use `currentColor` so they theme-adapt; full-colour Papirus SVGs will not adapt automatically.

---

## 1. Component map (file:line)

| Concern | Location |
|---|---|
| Boru example entry | `examples/iced_chat/main.rs`; registered `Cargo.toml:232-235` (`name = "boru"`, `required-features = ["gui"]`; `gui` at `Cargo.toml:209`, `video-playback` at `:210`, `terminal` at `:211`) |
| App state + messages + chat rendering | `examples/iced_chat/app.rs` (43,632 lines) |
| Download/video card renderer | `examples/iced_chat/download_progress_view.rs` (980 lines) |
| Design system | `design_tokens.rs`, `fonts.rs`, `card_shell.rs`, `icon_system.rs`, `ui_components.rs` |
| Icon system | `icon_system.rs` — `Icon` enum (`:76-133`), `IconSize` (`:199-221`), Lucide SVG bytes embedded at compile time (`:42-58`, `:139-191`); **no emoji, no file-type variants** |
| Legacy icon helper (bypasses Icon enum) | `app.rs:811-821` `icon_svg(bytes, size_px)`; used by `download_progress_view.rs` and app chrome |
| Shared by Me table | `examples/iced_chat/shared_by_me_table.rs` (1,806 lines) |
| Dashboard view models | `dashboard_view_model.rs`, `downloaded_view_model.rs`, `downloading_view_model.rs`, `peers_downloading_view_model.rs`, `recent_activity_view_model.rs`, `activity_log_view_model.rs` |
| File indexer / library ops (MIME guesser) | `file_library_ops.rs:227-269` `guess_mime_type`; `src/media_classification.rs` `classify_attachment` |
| Media classification (video) | `src/media_classification.rs:15-17` (`VIDEO_EXTENSIONS`), `:38-68` `classify_attachment` |
| Notification system | `examples/iced_chat/notification/{event,service,render,backend}.rs` |
| Native OS picker | `rfd::AsyncFileDialog` — `app.rs:10396-10408`, `13159-13184` (chat attach), `19319-19330` (share file), `19335-19348` (share folder), `19856-19858`; **must stay native** |

---

## 2. Every surface that renders a file/folder, and what icon it uses

### 2.1 Chat surfaces

| Surface | Renderer | Icon today | Notes |
|---|---|---|---|
| Incoming file card (shared file) | `app.rs:21433-21461` `set_pending_file` → `ChatEntry::system_download` → `view_download_attachment` `app.rs:7953-7993` → `download_progress_view.rs:273-737` | `ICON_FILES` (File kind) at `download_progress_view.rs:292`, drawn via `icon_svg(attachment_icon, TYPO_SM)` `:322` (14 px) | Card icon depends only on `TransferKind`, not extension/MIME |
| Outgoing file card (upload) | `app.rs:14599-14647` `ExecuteFileSend` → `system_download` card, `state=Active` | same card, `ICON_FILES` | body text `"Uploading: {filename}"` (`app.rs:14645`) |
| Video card (incoming) | `set_pending_file` `app.rs:21480-21486` (classify via `classify_attachment`) → `TransferKind::Video` | `ICON_ACTIVITY` at `download_progress_view.rs:290`; poster or `"VIDEO"` text placeholder `:489`; `"▶"` glyph `:514` | Video cards use the **activity** pulse icon, not a video/file icon |
| Video card (outgoing) | `ExecuteFileSend` `app.rs:14618-14623` (`is_video_file` → `TransferKind::Video`) | same as above | |
| Image attachment card | `ImageDownloaded` `app.rs:15719-15734` → inline image in chat log (`view_chat_log` image branch `app.rs:28864+`) | rendered image bytes, no type icon; error placeholder `"🖼 Image unavailable"` **emoji** `app.rs:28940` | |
| Audio attachment card | **none exists** — `TransferKind` has no Audio variant (`src/chat_callbacks.rs:60-68`) | — | audio files become generic File cards |
| Generic document card | same download card | `ICON_FILES` | |
| Download-progress card | `download_progress_view.rs:747-833` `progress_section` (progress bar), `:836-912` `action_buttons` | `ICON_FILES`/`ICON_ACTIVITY` header | |
| Failed-transfer card | `DownloadState::Failed` branch `download_progress_view.rs:414-466` | same header icon | error text + recovery hint |
| Re-shared file card | `ReshareFile` `app.rs:15657-15675` → re-encodes into `ExecuteFileSend` → new upload card | `ICON_FILES` | |
| System messages involving files | `src/chat_core.rs:1993` `push_system("{} shared a file: {}")` → rendered as plain centred text `app.rs:28826-28841` | **no icon** (by design, UI-29) | |
| Chat search results | `view_chat_search_panel` `app.rs:26936+` | **no icon**; `entry.body` text snippet | |

### 2.2 File Sharing dashboard

| Surface | Renderer | Icon today | Notes |
|---|---|---|---|
| Shared by Me — name cell | `shared_by_me_table.rs:735-799` `name_cell` | image/video → `file_thumbnail` (`ui_components.rs:2657`, `Icon::Image`/`Icon::Play`, 40 px box); other → `file_icon` `:801-815` | MIME prefix match only: `image/`→Image, `video/`+`audio/`→Play, `zip|tar|gzip`→Files, else Files |
| Shared by Me — kind label | `shared_by_me_table.rs:258-263` `kind_label` | text (raw MIME string or `"File"`) | used in row meta `:762-771` and details panel `:1199` |
| Shared by Me — share button | `:516-535` | `Icon::Upload` Xs (16 px) | |
| Shared by Me — empty state | `:1374-1402` `empty_body` | `Icon::Upload` Xl (28 px) | |
| Shared by Me — action menu | `:1007+` `action_menu` | `Icon::MoreVertical` Sm (18 px) `:977-985` | menu items are text-only |
| Shared by Me — details panel | `:1161-1210` `details_panel` | no icon; text `Name/Kind/Size/Shared on/Content ID/Source` | Kind = MIME string |
| Shared with Me | `app.rs:31692+` `view_shared_with_me`; rows `:31883-31902` | image/video → `file_thumbnail` (`Icon::Image`/`Icon::Play`); **other → empty `Space` at `FILE_THUMBNAIL_EDGE`, no icon** | MIME from `RemoteSharedFile` (`file.mime_type`) |
| Downloading — empty state | `app.rs:33487-33494` | `Icon::Files` Xl (28 px) via `empty_state` | |
| Downloading — row name line | `app.rs:33628-33632` (`incoming_download_row` `:33545+`) | `Icon::Files` Xs (16 px) | `IncomingTransferRow` has **no MIME** (`downloading_view_model.rs:121-142`) |
| Downloaded — empty state | `app.rs:33147-33154` | `Icon::Check` Xl (28 px) | |
| Downloaded — row name cell | `app.rs:33332-33338` (`downloaded_row` `:33253+`) | `Icon::Files` Md (20 px) **regardless of MIME** | `CompletedDownloadItem.mime_type` is available (`dashboard_view_model.rs:583`) but unused for icon; `type_label` shows truncated MIME text `:33262-33266` |
| Peers Downloading from Me — row file line | `app.rs:33002-33020` (`peer_download_row` `:32895+`) | `Icon::Files` Xs (16 px) | `PeerDownload` has **no MIME** (`dashboard_view_model.rs:188-202`) |
| Recent Download Activity — row | `app.rs:32544-32641` `recent_activity_row` | status icon only: Check/AlertTriangle/Activity Xs (16 px) — **not a file-type icon**; `file_label` is text | `RecentActivityRow` has no MIME (`recent_activity_view_model.rs:58-76`) |
| Activity Log (transfer history) — row | `app.rs:34067-34220` `activity_log_row` | **no file icon**; `item_label` text; detail panel uses `Icon::AlertTriangle` | `ActivityLogRow` has no MIME (`activity_log_view_model.rs`) |
| Search results (dashboard) | each tab filters its rows (`dashboard_filters.rs`, `app.rs:33461-33478` etc.) | inherits the tab's row icon | no icon change for search |
| File/folder summary rows | Shared by Me table only; no folder rows exist | `file_icon` `:801-815` | folder state unavailable (see §7) |

### 2.3 Sharing flows

| Surface | Renderer | Icon today | Notes |
|---|---|---|---|
| Selected-file summary after picker | `SharedFilePicked` `app.rs:19374+` → `sharing_status` text `"Registering {display_name}…"` `:19385-19386` | **no icon** (text only) | |
| Share confirmation | no separate confirmation dialog; share menu `shared_by_me_table.rs:439-448` (`SHARE_MENU_ITEMS`: "Share Files...", "Share Folder...") | `Icon::Upload` on the menu button `:519` | |
| Recipient-selection dialogs | `form_components.rs:959+` `SelectablePeerPicker` | peer avatars/chips, **no file icons** | |
| Folder-sharing summaries | `SharedFolderPicked` `app.rs:19350-19372` | **none** — system message `"“{name}” can't be shared as a folder yet…"` | folders unsupported |
| Re-share dialogs | none — `ReshareFile` direct `app.rs:15657-15675` | — | |
| Native OS picker | `rfd::AsyncFileDialog` everywhere | **native; do not restyle** | |

### 2.4 Other surfaces

| Surface | Renderer | Icon today | Notes |
|---|---|---|---|
| File transfer notifications | `emit_message_notification` `app.rs:20551-20602` | **emoji**: `"📎 {name}"` (FileShare), `"🖼️ Image"` (ImageShare) `:20571-20572` | |
| Notification rendering | `notification/render.rs:84-107` | text only (`"File transfer completed"` / `"sent a file"`) | `FileTransferCompleted`/`FileTransferFailed` kinds exist (`event.rs:40-42`) but **no emit site exists anywhere in the repo** (searched `src/`, `examples/`) — currently dead |
| Recent activity entries | dashboard card `app.rs:32409+` + home `view_recent_activity_card` `app.rs:24844-24920` | home card: `ICON_FILES` for `ActivityKind::FileShared` at `:24863`, 14 px `icon_svg`; dashboard card: status icons only | |
| Transfer-detail panels | `shared_by_me_table.rs:1161-1210`; activity-log detail panel `app.rs:34123-34159` | text; `Icon::AlertTriangle` for failure detail | |
| File-related search results | chat search `app.rs:26936+`; dashboard search | none / inherited | |
| Context menus showing file info | `shared_by_me_table.rs:1007+` action menu; row details | `Icon::MoreVertical`; text `Kind` line | |
| Compact sidebar transfer indicator | **none exists** | — | sidebar has File Sharing nav button (`Icon::Folder` Md `app.rs:22880-22886`) and Recent Activity home card |

---

## 3. Every component using emoji, generic symbols, or custom icons

### Emoji (file-related)
- `app.rs:20571` — `"📎 {name}"` file notification body (emoji file icon).
- `app.rs:20572` — `"🖼️ Image"` image notification body (emoji).
- `app.rs:28940` — `"🖼 Image unavailable"` image placeholder (emoji).
- `app.rs:799` — `pub(crate) const ICON_EMOJI: &str = "😊";` — dead constant, no usages.
- `app.rs:26343-26345` — reaction emoji picker (user content, not file icons).
- `dashboard_filters.rs:555` — `"🧾"` inside a test string (not rendered).
- `app.rs:15860` — `👤` in a comment only.

### Generic text glyphs
- `download_progress_view.rs:514` — `text("▶")` play button glyph (28 px).
- `download_progress_view.rs:489` — `text("VIDEO")` placeholder.
- `app.rs:15922` — `entry.body = format!("Shared: {name} ✓")` — check mark in upload card body.
- `app.rs:31569`, `app.rs:35166` — `"✓"` text in buttons.
- `app.rs:28741-28743` — braille spinner `⠋⠙…` for link-preview loading (not file-related).

### Custom icons (Lucide SVG)
All file/media-related icons in `icon_system.rs` are Lucide line icons used generically:
- `Icon::Files` → `assets/icons/lucide/files.svg` (`icon_system.rs:163`) — generic two-document glyph; used for every non-media file row/card.
- `Icon::Image` → `image.svg` (`:166`) — image rows.
- `Icon::Play` → `play.svg` (`:167`) — video/audio rows (and the video player).
- `Icon::Upload` → `upload.svg` (`:168`) — share buttons, empty states.
- `Icon::Folder` → `folder.svg` (`:164`) — **only** the sidebar File Sharing nav button (`app.rs:22880`); never used for a folder row.
- `Icon::Paperclip` → `paperclip.svg` (`:165`) — composer attach button (`app.rs:29087`).
- Legacy constants in `app.rs:774-806` (`ICON_FILES`, `ICON_ACTIVITY`, `ICON_PAPERCLIP`, …) bypass the `Icon` enum via `icon_svg` (`app.rs:811-821`); `ICON_ACTIVITY` (an activity/pulse glyph) is used as the header icon for **both** Image and Video download cards (`download_progress_view.rs:289-293`).

---

## 4. Shared components that should be consolidated

| Component | Location | State | Consolidation value |
|---|---|---|---|
| `Icon` enum + `IconSize` | `icon_system.rs:76-221` | single source of truth for semantic icons | PAPIRUS should add `FileType` icons here or a sibling module |
| `icon_svg(bytes, px)` legacy helper | `app.rs:811-821` | parallel path to `Icon::build()`; `download_progress_view.rs:322` uses it | migrate call sites to `Icon::build()` so colour/size tokens are consistent |
| `file_thumbnail` | `ui_components.rs:2657-2702`, `FILE_THUMBNAIL_EDGE = 40.0` (`:2648`) | already shared by Shared by Me + Shared with Me | keep; add a file-type fallback icon parameter |
| `FileIdentityCell` | `ui_components.rs:1988-2046` | shared, but `app.rs:33332-33338` inlines icon+name instead (borrow-lifetime comment `:33329-33331`) | refactor to owned strings so Downloaded table reuses it |
| `file_icon` | `shared_by_me_table.rs:801-815` | **private** MIME→Icon resolver | promote to central `file_type_icon` used by all tables |
| `kind_label` | `shared_by_me_table.rs:258-263` | MIME→label text helper | promote to central MIME→category label |
| `empty_state` | `ui_components.rs:984-1035` | shared; takes an `Icon` | fine as-is |
| `CardShell` | `card_shell.rs:101+` | dashboard cards | download card is bespoke (`download_progress_view.rs`); not worth forcing into CardShell |
| Download card header icon | `download_progress_view.rs:289-293` | bespoke `TransferKind`→byte-const map | replace with `Icon::Files`/`Icon::Image`/`Icon::Play` + future file-type icon |

---

## 5. Duplicate file-type resolvers / extension maps

| # | Resolver | Location | Coverage | Used by |
|---|---|---|---|---|
| 1 | `classify_attachment(mime, filename)` + `VIDEO_EXTENSIONS` (9 entries: 3gp avi flv m4v mkv mov mp4 webm wmv) | `src/media_classification.rs:15-17`, `:38-68` | video vs non-video only, MIME+extension conservative | `app.rs:2456-2458` `ChatEntry::is_video_file`; `app.rs:21480` `set_pending_file`; `app.rs:14618` upload |
| 2 | `guess_mime_type(filename)` — extension→MIME map (~35 entries incl. png/jpg/gif/webp/svg/bmp/ico/mp4/webm/mkv/avi/mov/mp3/ogg/wav/flac/aac/opus/pdf/zip/gz/tar/rar/7z/json/xml/csv/html/txt/md/yaml/toml) | `file_library_ops.rs:227-269` | broad extension→MIME | file library import (`:439`, `:542`, `:743`) |
| 3 | inline extension→MIME map (9 entries: txt md json pdf png jpg jpeg gif webp) | `app.rs:19436-19452` (`SharedFilePicked`) | narrow | dashboard "Share Files..." flow |
| 4 | inline image-extension check (png jpg jpeg gif webp bmp) | `app.rs:13168-13173` (`AttachPressed`) | image vs file | chat attach flow |
| 5 | MIME-prefix→Icon map (`image/`→Image, `video/`/`audio/`→Play, `zip|tar|gzip`→Files, else Files) | `shared_by_me_table.rs:743-754` + `:801-815` | icon selection | Shared by Me |
| 6 | MIME-prefix→Icon map (`image/`→Image, `video/`→Play, else none) | `app.rs:31887-31893` | icon selection | Shared with Me |
| 7 | `TransferKind`→icon-bytes map (`Image`/`Video`→`ICON_ACTIVITY`, `File`→`ICON_FILES`) | `download_progress_view.rs:289-293` | icon selection | chat download card |

There is **no extension normalization** (case, compound `.tar.gz`, leading dots) anywhere except `media_classification.rs` (lowercase trim, `rsplit_once('.')`) — PAPIRUS Tasks 5/6 need a single resolver to replace items 3-7 and reuse items 1-2.

---

## 6. MIME type availability

### Available (UI row models carry `mime_type`)
- `SharedItem.mime_type: Option<String>` — `dashboard_view_model.rs:110` (from `RemoteSharedFile` `:178`, local `FileObject` `:382`, `:395`; `DownloadRow` `:302`; `CompletedDownloadItem` `:583`, `:612`).
- `SharedByMeRow.mime_type: Option<String>` — `shared_by_me_table.rs:76`, projected `:179` from `FileObject`.
- `DownloadedItem.mime_type: String` — `downloaded_view_model.rs:87`, projected `:117` from `CompletedDownloadRecord`.
- Chat: `classify_attachment` accepts `Option<&str>` MIME (`src/media_classification.rs:38`), but chat cards never store MIME (see below).

### NOT available (only filename / extension / label)
- **Chat download card** `DownloadAttachment` (`app.rs:1909-1932`): fields are `kind: TransferKind`, `name`, `ticket`, `state`, `source_peer`, `speed`, `thumbnail*`, `poster_dimensions`, `playback_error`, `expected_content_hash` — **no MIME field**. Only `TransferKind` (from extension classification) exists.
- **Downloading tab** `IncomingTransferRow` (`downloading_view_model.rs:121-142`): `display_name` only (enrichment map keyed by item id `:184-207`); no MIME.
- **Peers Downloading from Me** `PeerDownload` (`dashboard_view_model.rs:188-202`): `display_name` only.
- **Recent Download Activity** `RecentActivityRow` (`recent_activity_view_model.rs:58-76`): `file_label` only.
- **Activity Log** `ActivityLogRow` (`activity_log_view_model.rs`): `file_label` only.
- **Chat search results**: `entry.body` text only (`app.rs:26995-27005`).

---

## 7. Folder state availability

- **No UI row model carries folder state.** There is no `is_directory`/`is_folder`/`kind == folder` field on `SharedByMeRow`, `SharedItem`, `DownloadRow`, or any transfer row.
- Folder sharing is explicitly unsupported: `AppMessage::SharedFolderPicked` (`app.rs:19350-19372`) prints a system message that the secure catalogue shares individual files only.
- `Icon::Folder` exists (`icon_system.rs:103,164`) but is used **only** for the sidebar File Sharing nav button (`app.rs:22880-22886`), not for any file/folder row.
- The native folder picker (`app.rs:19335-19348`, `AddSharedFolder`) and the `FileIndexer` (`src/file_indexer.rs:582,650` `metadata.is_dir()`) know about directories internally, but that state never reaches the UI.
- `SharedFolderPicked` extracts only the display name (`app.rs:19362-19366`); no folder row is created.

---

## 8. Existing icon-size conventions (px values used today)

| Token / constant | px | Defined | Used for file rows |
|---|---|---|---|
| `IconSize::Xs` | 16 | `icon_system.rs:215` | Downloading row name (`app.rs:33628`), Peers Downloading file line (`app.rs:33004`), recent-activity status (`app.rs:32586`), dashboard file lines (`app.rs:33004`) |
| `IconSize::Sm` | 18 | `icon_system.rs:217` | action menu MoreVertical (`shared_by_me_table.rs:979`) |
| `IconSize::Md` | 20 | `icon_system.rs:219` | `file_icon` (`shared_by_me_table.rs:811`), `FileIdentityCell` (`ui_components.rs:2011`), Downloaded name cell (`app.rs:33336`), `file_thumbnail` fallback (`ui_components.rs:2671`) |
| `IconSize::Lg` | 24 | `icon_system.rs:221` | (quick actions, home cards — not file rows) |
| `IconSize::Xl` | 28 | `icon_system.rs:223` | `empty_state` icons (`ui_components.rs:994`), Shared by Me empty (`shared_by_me_table.rs:1379`) |
| `TYPO_SM` | 14 | `fonts.rs:220` | legacy `icon_svg` calls incl. download card header (`download_progress_view.rs:322`), Recent Activity home (`app.rs:24880`) |
| `FILE_THUMBNAIL_EDGE` | 40 | `ui_components.rs:2648` | image/video thumbnail box (Shared by Me, Shared with Me) |
| `▶` play glyph | 28 | `download_progress_view.rs:514` | video card overlay |
| `QUICK_ACTION_ICON_SIZE` | 56 | `quick_actions.rs:27` | home quick-action icon container (not file rows) |

PAPIRUS Task 4 semantic sizes map onto today's tokens as: compact ≈ Xs/Sm (16-18), list ≈ Md (20), card ≈ FILE_THUMBNAIL_EDGE (40) / Md (20), large ≈ Xl (28), hero ≈ 40-56 (needs a new token; nothing renders file icons at 48-64 today).

---

## 9. Light and dark theme support

- **Yes.** `IcedChat.dark_mode: bool` (`app.rs:238`, default `false` in `main.rs:1425`); toggled via `AppMessage::ToggleDark` (`app.rs:5387`, handled `app.rs:18816`) and GUI test action.
- `IcedChat::theme()` (`app.rs:21815-21819`) and `theme_from_dark` (`app.rs:21824-21828`) map to `iced::Theme::Dark`/`Light`; `main.rs:1672-1675` wires it to the iced app.
- `design_tokens.rs:328-330` `fn dark(theme) -> bool`; every colour accessor branches on `Theme::Dark`/`Light` (e.g. `surface` `:353-359`, `text_primary` `:402-409`, `primary` `:429-436`).
- `icon_system.rs` icons are SVG `currentColor` with `svg::Style` colour callbacks (`icon_system.rs:303-324`), so they re-colour per theme. **Full-colour Papirus SVGs would not adapt** — the PAPIRUS implementation must either ship theme-aware colouring or accept fixed-colour icons, and licensing must permit theming changes (GPL gate, spec lines 30-46).

---

## 10. Key findings for the PAPIRUS implementation cards

- **F1 — No file-type icons exist anywhere.** Every surface uses Lucide generics (`Icon::Files`/`Icon::Image`/`Icon::Play`/`Icon::Upload`) or nothing.
- **F2 — Five independent resolvers** (§5 items 3-7) must collapse into one `resolve_file_icon`; `guess_mime_type` (`file_library_ops.rs:227`) and `classify_attachment` (`media_classification.rs`) can be reused as building blocks.
- **F3 — Chat `DownloadAttachment` carries no MIME.** Adding one requires touching `app.rs:1909-1932`, `:1952-1987` (`new()`), `set_pending_file` `:21433-21461`, `ExecuteFileSend` `:14599+`, and history persistence (check `ChatEntry::system_download` `app.rs:2412+` and the stored `DownloadAttachment`).
- **F4 — Transfer projections lack MIME** (`IncomingTransferRow`, `PeerDownload`, `RecentActivityRow`, `ActivityLogRow`); icon resolution there must fall back to filename-extension resolution (or projection enrichment).
- **F5 — Folder state is absent from the UI** and folder sharing is unsupported; a Papirus folder icon currently has no row to attach to. Do not build folder rows in this epic unless scope adds folder sharing.
- **F6 — Emoji file icons exist in exactly two runtime paths** (`app.rs:20571-20572` notifications, `app.rs:28940` image-unavailable) and one dead constant (`app.rs:799`).
- **F7 — The download card header icon is wrong for media** (`ICON_ACTIVITY` pulse used for Image/Video, `download_progress_view.rs:289-293`); a file-type icon pass should replace it.
- **F8 — "Shared with Me" non-media rows have no icon at all** (`app.rs:31897-31901`) — a visible gap a Papirus fallback would fill.
- **F9 — The native OS picker is `rfd` everywhere and must remain native.**
- **F10 — `FileTransferCompleted`/`FileTransferFailed` notification kinds are dead code** (never emitted) — PAPIRUS icon work there is currently a no-op surface.

---

## 11. Baseline evidence

- Repo: `origin` → `dmahony/boru.git`, branch `wt/t_50f26f47` (worktree `/home/dan/iroh-gossip-chat/.worktrees/t_50f26f47`).
- Analysis commit: `<recorded after landing>` — this note only; no production code changed.
- Build (acceptance criterion): `cargo build --bin boru --features gui,video-playback,terminal` — result recorded in the task handoff (exit code + duration).
