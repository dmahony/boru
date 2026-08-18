# BORU-UI-01 — Existing UI Constants Audit

Task: `t_38a58f9d` — first task of the Live UI Editor chain (Boru_Live_UI_Editor_Agent_Tasks.pdf).
Purpose: inventory of hard-coded visual values in the Boru GUI that will move into the typed
`BoruTheme` model (BORU-UI-02+). This is a **map only** — no behaviour or appearance changed.

Audit scope: `src/bin/boru/` (the Iced GUI) plus behavioural constants in `src/` that must
stay out of the theme system. All line numbers are as of commit `c275d8e2` (origin/main).

---

## 1. Summary

Boru already has a substantial token layer (`design_tokens.rs`, `fonts.rs`, `icon_system.rs`,
`card_shell.rs`, `presentation.rs`). Most **colours, radii, avatar sizes and core layout widths**
are centralized. The remaining raw literals are concentrated in:

- **per-component fixed dimensions** — column widths, row heights, thumbnail sizes, gap values
  (app/files.rs, shared_by_me_table.rs, status_card.rs, video_file_card.rs, app/home.rs,
  app/chat.rs)
- **text sizes** — a long tail of `.size(N)` / `text(...).size(N)` calls that bypass `TypeRole`
- **semantic colour gaps** — status/state colours still written as raw `Color::from_rgb(...)`
  literals (download states, online dots, pending-request amber, error reds)
- **dialog / call / media overlays** — opaque black backdrops and media-frame colours

Raw-literal density per file (counts of matches for the audited patterns — padding, spacing,
`Length::Fixed`, text `.size()`, `Border { radius/width }`, `Color::from_rgb/rgba`):

| File | paddings | spacings | Fixed lengths | text sizes | radii | border widths | raw colours |
|---|---|---|---|---|---|---|---|
| app/chat.rs | 8 | 5 | 23 | 4 | 6 | 13 | several |
| app/files.rs | 0 | 31 | 28 | 2 | 0 | 2 | 0 |
| app/home.rs | 0 | 16 | 7 | 1 | 0 | 0 | 0 |
| app/sidebar.rs | 3 | 1 | 6 | 0 | 3 | 2 | 4 |
| status_card.rs | 0 | 8 | 13 | 0 | 2 | 9 | 3 |
| shared_by_me_table.rs | 0 | 10 | 8 | 2 | 2 | 9 | 0 |
| app/discover.rs | 1 | 2 | 4 | 0 | 0 | 6 | 8 |
| app/dialogs.rs | 1 | 2 | 1 | 2 | 1 | 2 | 4 |
| app/calls.rs | 0 | 0 | 8 | 5 | 3 | 0 | 0 |
| app/contacts.rs | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| app/settings.rs | 0 | 0 | 3 | 0 | 2 | 2 | 8 |
| app/tunnels.rs | 1 | 0 | 0 | 0 | 0 | 0 | 0 |
| app/groups.rs | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| video_file_card.rs | 0 | 0 | 8 | 0 | 1 | 2 | 4 |
| download_progress_view.rs | 0 | 0 | 2 | 0 | 0 | 6 | 6 |
| ui_components.rs | 2 | 6 | 4 | 1 | 4 | 7 | 0 |
| form_components.rs | 0 | 0 | 2 | 1 | 0 | 0 | 0 |
| quick_actions.rs | 0 | 1 | 1 | 2 | 0 | 0 | 1 |
| component_gallery.rs | 0 | 32 | 27 | 1 | 0 | 0 | 0 |
| app.rs | 0 | 0 | 10 | 3 | 2 | 11 | 10+ |
| offscreen_status_card.rs | 0 | 5 | 5 | 0 | 0 | 0 | 1 |

---

## 2. Existing token infrastructure (already centralized)

These modules are the foundation the typed theme will absorb; most of their contents map
1:1 onto `BoruTheme` fields.

| Module | Provides | Becomes |
|---|---|---|
| `design_tokens.rs` | Colour accessors (`color_canvas`, `surface`, `text_primary`, `primary`, `border_muted`, `color_success/danger/warning/focus`, soft tints, bubble bg/border, status-card palette…), spacing scale `SPACE_2..SPACE_40`, control heights, radii `RADIUS_SM/MD/LG/XL/CARD`, `BORDER_WIDTH`, `FOCUS_WIDTH`, avatar sizes (`AVATAR_SM/MD/LG/PROFILE/CHAT_LIST/CHAT_HEADER/MSG`, status dots), layout dims (`SIDEBAR_WIDTH*`, `DETAILS_PANEL_WIDTH`, `MESSAGE_MAX_WIDTH`, `CHAT_BUBBLE_*`, `IMAGE_PREVIEW_*`), responsive thresholds, shadow builders, `card_style`/`elevated_style`/`dialog_style`/`surface_style`/`focus_border`/`icon_button` | `ColorTokens`, `SpacingTokens`, `RadiusTokens`, `SidebarTheme`, `ChatTheme`, `ShadowTokens` |
| `fonts.rs` | `TypeRole` enum — 15 semantic typography roles with `family_name()` / `weight()` / `size_px()`; fallback font chain; legacy `sizes` aliases (`XL 28`, `LG 18`, `MD 15`, `SM 14`, `XS/XXS 12`, `HOME_SUBTITLE 16`, `DIALOG_TITLE 26`, `DIALOG_SUBTITLE 14`) | `TypographyTokens` (roles map to theme fields) |
| `icon_system.rs` | `IconSize` enum — `Xs 16 / Sm 18 / Md 20 / Lg 24 / Xl 28`; embedded Lucide SVGs | `IconTokens` (sizes) |
| `presentation.rs` | `chat_bubble_max_width` (560 px / 68 % rule), `MESSAGE_GROUP_WINDOW_MS`, `initials_color` (HSL-derived avatar colour), relative-time formatters, truncation helpers | stays as presentation helpers; **group window stays OUT** (see §4) |
| `card_shell.rs` | `CARD_ROW_HEIGHT 48`, `PEER_ROW_HEIGHT 60`, `DEFAULT_LIST_MAX_HEIGHT 180` | `ListTokens` / `AttachmentTheme` |

---

## 3. Raw literals remaining, by component

Format: `file:line — semantic role → recommended BoruTheme token`.

### 3.1 Sidebar / global shell

| Location | Role | Recommended token |
|---|---|---|
| app/sidebar.rs:247, 442, 449, 456, 474, 481, 924, 953, 971, 1680, 1920 — `iced::Padding { … }` literals | sidebar section / row padding | `SidebarTheme::section_padding`, `row_padding` |
| app/sidebar.rs:231, 470, 1227, 1782 — `radius: 0.0` | flat/no-radius rows | `RadiusTokens::none` |
| app/sidebar.rs:1097 — `border_radius: Some(10.0)` | hover pill radius | `SidebarTheme::item_radius` |
| app/sidebar.rs:1119–1120, 1539–1540, 1559–1560 — `Length::Fixed(24.0)` | 24 px utility/status icons | `IconTokens::sidebar` |
| app/sidebar.rs:1564 — `radius: 12.0` | avatar container radius | `RadiusTokens::avatar` |
| app/sidebar.rs:969, 1152, 1265, 1548 — raw `Color::from_rgb/rgba` (online dot red fallback, transparent hover, avatar colour) | status + avatar colours | `ColorTokens::status`, `ColorTokens::avatar` |
| app.rs:6943 — `SIDEBAR_NAME_SIZE = 15.0` | sidebar name text | `TypographyTokens::sidebar_name` |
| ui_components.rs:1394 — `SIDEBAR_SECTION_LABEL_SIZE = 11.0` | section label text | `TypographyTokens::section_label` |
| ui_components.rs:1605 — `SIDEBAR_FADE_FRAMES = 5` | appearance animation frames | `MotionTokens::sidebar_fade_frames` (presentation, low priority) |

### 3.2 Home dashboard

| Location | Role | Recommended token |
|---|---|---|
| app/home.rs:139 — `PEERS_BODY_MIN = 128.0`; :523/:618 — `ACTIVITY_ROW_HEIGHT = 32.0` | peers/activity card geometry | `HomeTheme::peers_body_min`, `activity_row_height` |
| app/home.rs:775 — `Length::Fixed(40.0)` | hero/status spacing | `HomeTheme::hero_gap` |
| app/home.rs:1139 — `.size(crate::fonts::HOME_SUBTITLE)` (16) | greeting subtitle | `TypographyTokens::home_subtitle` |
| app/home.rs:1432–1445 — `card_gap` spacers | quick-action grid gap | `HomeTheme::quick_action_gap` |
| app/home.rs:740 — `height(Length::Fixed(1.0))` divider | 1 px divider | `BorderTokens::hairline` |
| quick_actions.rs:32, 41, 44, 47 — `QUICK_ACTION_ICON_SIZE 40`, `TITLE_SIZE 16`, `DESC_SIZE 14`, `LINE_HEIGHT 1.45` | quick action card | `HomeTheme::quick_action_*` |
| quick_actions.rs:226–228 — `rgba(0,0,0,0.10)` blur 8 shadow | quick-action shadow | `ShadowTokens::card` (already exists as `shadow_card`) |
| status_card.rs:80–100 — `STATUS_CARD_TEXT_MIN_WIDTH_MEDIUM 260`, icon/text gaps 24/20 | status card layout | `HomeTheme::status_card_*` |
| status_card.rs:103, 110 — `STATUS_WARNING`, `STATUS_DANGER` | status card state colours | `ColorTokens::warning`, `ColorTokens::danger` |
| status_card.rs:279/286/288/290/297, 508–509, 513, 567 — Fixed 16/18/14/22/28/44×3, radius 1.5 / 14 | status-card gaps and bars | `HomeTheme::status_card_*` |
| design_tokens.rs STATUS_CARD_* constants | already tokenized ✓ | — |

### 3.3 Chat message list + composer

| Location | Role | Recommended token |
|---|---|---|
| app/chat.rs:26, 57 — `.size(40.0)` | spinner container | `ChatTheme::spinner_size` |
| app/chat.rs:512 — `.width(180.0)` | gif/emoji picker column | `DialogTheme::picker_width` |
| app/chat.rs:562–563, 615–616, 990–991 — `width: 1.0`, `radius: 8.0`, `border_radius: 8.0` | picker cell borders | `RadiusTokens::sm`, `BorderTokens::hairline` |
| app/chat.rs:606 — `Length::Fixed(160.0)` (emoji scroll), :979 — `Fixed(300.0)` (gif scroll) | picker panel heights | `DialogTheme::picker_height` |
| app/chat.rs:906–907, 938 — `Length::Fixed(150.0/100.0)` | gif thumbnails | `AttachmentTheme::gif_thumbnail` |
| app/chat.rs:491, 494 — `SCREEN_SHARE_VIDEO_W 640 / H 360` | screen-share video box | `ChatTheme::screen_share_size` |
| app/chat.rs:135–224 — raw `rgba(0,0,0,…)` overlay/shadow colours (0.45/0.25/0.35/0.15/0.55/0.3) | video control overlays | `ColorTokens::media_overlay` |
| app/chat.rs:1067, 1138, 1701, 2079… — `TYPO_SM` (alias of `fonts::SM` 14), `TYPO_XS` (12) | legacy typography aliases in chat chrome | migrate to `TypeRole` (ChatMetadata/ChatSender) |
| app.rs:483 — `LG as TYPO_LG, MD as TYPO_MD, …` alias block | legacy type aliases | `TypographyTokens` (remove aliases at migration) |
| bubble geometry (app/chat.rs:3223 bubble_border, 3274 max_width) | already tokenized ✓ | — |
| composer (app/chat.rs `view_composer`, ~1474+) | mostly tokenized via `SPACE_*` ✓ | `ChatTheme::composer_*` for any remaining literals |

### 3.4 File cards / file dashboard

| Location | Role | Recommended token |
|---|---|---|
| app/files.rs:2242 — `Fixed(72.0)`; :2250/:2260/:3064 — `Fixed(120.0)`; :2607/:2748/:3058/:3566 — `Fixed(140.0)`; :2616/:2622/:2754 — `Fixed(100.0)`; :3070/:3534/:3572 — `Fixed(110.0)`; :3525 — `Fixed(90.0)`; :3460/:3474 — `Fixed(80.0)`; :3976 — `Fixed(240.0)` | file table column widths | `AttachmentTheme::table_columns` |
| app/files.rs:1172 — `Fixed(200.0)` | empty-state art box | `AttachmentTheme::empty_state_height` |
| app/files.rs:3575 — `Fixed(4.0)` spacer | column spacer | `SpacingTokens::space_4` |
| shared_by_me_table.rs:401–405 — `COL_SHARED_WITH 144`, `COL_SIZE 64`, `COL_SHARED_ON 122`, `COL_DOWNLOADS 80`, `COL_ACTIONS 36` | shared-table column widths | `AttachmentTheme::shared_table_columns` |
| shared_by_me_table.rs:531 — `Fixed(176.0)`; :974–975 — `16×16`; :981 — `radius 8`; :1250 — `Fixed(96.0)` | chips / avatars | `AttachmentTheme::chip_*`, `RadiusTokens::sm` |
| shared_by_me_table.rs:539, 1005, 1025, 1138, 1186, 1228 — `width: 1.0` | table row borders | `BorderTokens::hairline` |
| download_progress_view.rs:56 — `PROGRESS_BAR_GIRTH 6`; :61 — `PCT_LABEL_WIDTH 44`; :83/:86/:89 — `PROGRESS_SLOT_HEIGHT 20`, `DETAIL_SLOT_HEIGHT 18`, `POLICY_SLOT_HEIGHT 30`; :116 — `BUTTON_LINE 30` | progress row geometry | `AttachmentTheme::progress_*` |
| download_progress_view.rs:252, 256, 518, 558, 592 — raw state colours (Temporary amber, Cancelled grey…) | download state colours | `ColorTokens::status_*` |

### 3.5 Video cards

| Location | Role | Recommended token |
|---|---|---|
| video_file_card.rs:139, 143 — `NARROW_CARD_BREAKPOINT 560`, `MEDIUM_CARD_BREAKPOINT 780` | responsive bands | `AttachmentTheme::video_breakpoints` |
| video_file_card.rs:310, 315 — `MEDIA_FRAME_BACKGROUND`, `ON_MEDIA_TEXT` | media frame colours | `ColorTokens::media` |
| video_file_card.rs:486 — `MEDIA_FRAME_RADIUS 13` | media frame radius | `RadiusTokens::media_frame` |
| video_file_card.rs:490, 494 — `MEDIA_FRAME_BORDER` rgba(1,1,1,0.10), `MEDIA_FRAME_OVERLAY_BG` rgba(0,0,0,0.62) | media frame border/overlay | `ColorTokens::media_border`, `media_overlay` |
| video_file_card.rs:498 — `PLAY_OVERLAY_SIZE 64` | play button overlay | `AttachmentTheme::video_play_overlay` |
| video_file_card.rs:476 — `HEADER_FILENAME_MAX_WIDTH 420` | header layout cap | `AttachmentTheme::video_header_width` |
| video_file_card.rs:1289 — `Length::Fixed(90.0)` | small control sizing | `AttachmentTheme::video_controls` |

### 3.6 Public rooms / discover

| Location | Role | Recommended token |
|---|---|---|
| app/discover.rs:491, 495, 498, 1474, 1560 — raw greys `rgb(0.4,0.4,0.4)`, `rgba(0.3,0.3,0.3,…)` | tag/status surfaces | `ColorTokens::surface_muted`, `RoomTheme::tag_*` |
| app/discover.rs:1464 — `rgb(0.8,0.2,0.2)` | error accent | `ColorTokens::danger` |
| app/discover.rs:507, 1096, 1408, 1495, 1957, 1972 — `width: 1.0` | card/row borders | `BorderTokens::hairline` |
| app/discover.rs:706–707 — progress `length 80 / girth 6` | progress bar | `RoomTheme::progress_*` / `AttachmentTheme` |
| app/discover.rs:1459 — `Length::Fixed(200.0)` | room banner | `RoomTheme::banner_width` |
| app/discover.rs:429, 430 — `CATALOGUE_ROW_HEIGHT 52`, `OVERSCAN 800` (local consts) | catalogue rows | `RoomTheme::row_height` |

### 3.7 Friends / contacts

| Location | Role | Recommended token |
|---|---|---|
| app/contacts.rs:107, 326 — `width: 1.0` | row borders | `BorderTokens::hairline` |
| app/contacts.rs:112 — `rgb(0.6,0.6,0.6)` fallback avatar | avatar colour | `ColorTokens::avatar_fallback` |
| app/contacts.rs:314 — `rgb(0.7,0.6,0.0)` | pending-request amber | `ColorTokens::warning` |

### 3.8 Tunnels

| Location | Role | Recommended token |
|---|---|---|
| app/tunnels.rs:192 — `padding([2, 6])` | tunnel status chip | `TunnelTheme::chip_padding` |
| (rest of tunnel UI already token-driven ✓) | | |

### 3.9 Dialogs

| Location | Role | Recommended token |
|---|---|---|
| app/dialogs.rs:26 — `Fixed(72.0)` ×2, :27 — `.size(48)` | dialog avatar | `DialogTheme::avatar_size` |
| app/dialogs.rs:28 — `.size(22)` / `.size(15)` | dialog title/body text | `TypographyTokens::dialog_title`, `Body` |
| app/dialogs.rs:31 — `.spacing(12)`, :32 — `.padding(32)` | dialog spacing/padding | `DialogTheme::padding`, `SpacingTokens` |
| app/dialogs.rs:34–35 — raw `rgb(0.12,0.13,0.17)`, `rgb(0.35,0.38,0.45)`, `radius 16.0` | dark dialog panel | `DialogTheme::panel`, `RadiusTokens::lg` |
| app/dialogs.rs:41 — `rgba(0,0,0,0.72)` backdrop | backdrop | `ColorTokens::dialog_backdrop` (design_tokens has `dialog_backdrop()` ✓ — replace raw) |
| app/dialogs.rs:724 — `padding([6, 14])`, :726 — `.spacing(12)` | dialog controls | `DialogTheme::control_*` |
| boru_dialog.rs | already token-driven (`design_tokens::dialog_style`, `SPACE_*`) ✓ | — |

### 3.10 Calls

| Location | Role | Recommended token |
|---|---|---|
| app/calls.rs:29 — `.size(36.0)`, :30–33 — `96×96`, :36 — `radius 48` | call avatar | `CallTheme::avatar_size`, `RadiusTokens::avatar` |
| app/calls.rs:47 — `Fixed(40.0)`; :64 — `.size(44/18)`; :138 — `.size(26/22/16)` | call status/name/duration text | `TypographyTokens::call_*` |
| app/calls.rs:106–111, 127–132 — `220×150` PiP, `radius 12` | local PiP frame | `CallTheme::pip_size`, `RadiusTokens::md` |

### 3.11 Shared widgets (ui_components.rs, form_components.rs)

| Location | Role | Recommended token |
|---|---|---|
| ui_components.rs:912 — `.spacing(2.0)` | tight row gap | `SpacingTokens::space_2` |
| ui_components.rs:1220, form_components.rs:979 — `.size(10.0)` | badge/label text | `TypographyTokens::badge` |
| ui_components.rs:819, 2011, 2045 — `radius: 0.0`; :2044 — `width: 2.0` | flat rows / focus ring | `RadiusTokens::none`, `BorderTokens::focus` |
| ui_components.rs:352–353, 712–716, 1154–1159, 1249–1290 — `Length::Fixed(self.size)` avatar/badge math | parameterized avatars | `AvatarTokens` (already parameterized — move base sizes to theme) |

### 3.12 app.rs one-off style functions

| Location | Role | Recommended token |
|---|---|---|
| app.rs:718–719 — `IMAGE_PREVIEW_MAX_WIDTH 360 / MAX_HEIGHT 400` | **duplicates** `design_tokens.rs` same-name constants | consolidate into `ChatTheme`/`AttachmentTheme` |
| app.rs:720 — `ATTACHMENT_RADIUS 10` | attachment thumb radius | `RadiusTokens::attachment` |
| app.rs:6968 — `PROFILE_HEADER_AVATAR_SIZE = AVATAR_PROFILE` | already aliases token ✓ | — |
| app.rs:1231–1263 — hover/pressed colour math (`base * 0.85` / `* 1.2`) | button state derivation | `ColorTokens` (derive from `primary`) |
| app.rs:2586–2593 — download state colours (`rgb(0.2,0.7,0.2)`, `rgb(0.78,0.58,0.16)`, `rgb(0.8,0.22,0.22)`, `rgb(0.55,0.55,0.55)`) | state colours | `ColorTokens::status_*` |
| app.rs:1157, 1190, 1211, 1310, 1328, 18730–18759, 18808 — raw greys `rgb(0.5,0.5,0.5)` / `rgb(0.6,0.6,0.6)` / `rgb(0.4,0.4,0.4)` | disabled/muted glyphs | `ColorTokens::text_muted`, `surface_muted` |
| app/settings.rs:474–476, 1192–1194, 1246–1248, 1379–1381 — raw reds/greens (`rgb(0.6,0.15,0.15)`, `rgb(0.15,0.55,0.2)`…) | settings status colours | `ColorTokens::danger`, `success` |
| app/settings.rs:403 — `Fixed(52.0)`; :999 — `Fixed(160.0)`; :927–930 — `border_radius 8.0` / `4.0` | settings controls | `ControlTokens`, `RadiusTokens` |

### 3.13 Component gallery (component_gallery.rs)

32 `.spacing(...)` and 27 `Length::Fixed(...)` literals. Dev-only preview screen; it should adopt
the theme tokens when BORU-UI-14 (gallery) lands, but extracting its literals is **not** a
priority — it exists to preview the tokenized components.

### 3.14 Offscreen test helper (offscreen_status_card.rs)

`Fixed(140/160/80/40)` at 452/455/515/518 and the `#F7F9F8` literal at 106 mirror the home
status card for screenshots. Low priority; should share `HomeTheme::status_card_*` when that
exists.

---

## 4. Behavioural constants — OUT of scope (must NOT move into the theme)

These control protocol behaviour, limits, timeouts or data logic. Verified during the audit and
explicitly excluded per the PDF guardrails:

| Location | Constants | Why out |
|---|---|---|
| src/catalogue_limits.rs | `MAX_CATALOGUE_REQUEST_BYTES` (256 KiB), `MAX_CATALOGUE_RESPONSE_BYTES` (4 MiB), `MAX_CATALOGUE_PAGE_BYTES` (1 MiB), `MAX_CATALOGUE_FILES` (10k), `MAX_COLLECTIONS` (1k), `MAX_ENTRIES_PER_COLLECTION` (10k), `MAX_CATALOGUE_PAGE_SIZE` (500), `MAX_INVALID_RESPONSE_ATTEMPTS` (3), `MAX_FILE_SIZE_BYTES` (10 TiB) | protocol/limits |
| src/catalogue_model.rs | `MAX_SHARED_FILE_ID_LENGTH` 256, `MAX_DISPLAY_NAME_LENGTH` 512, `MAX_DESCRIPTION_LENGTH` 1024, `MAX_MIME_TYPE_LENGTH` 128, `MAX_CONTENT_HASH_LENGTH` 128, `MAX_COLLECTION_IDS` 256, `MAX_COLLECTION_ID_LENGTH` 256, `MAX_COLLECTION_NAME_LENGTH` 512, `MAX_TIMESTAMP_FUTURE_SKEW_MS` (24 h) | wire-format limits |
| src/backfill.rs | `DEFAULT_MAX_BACKFILL` 50, `BACKFILL_REQUEST_TIMEOUT` 5 s, `SERVER_MAX_BACKFILL` 50, `CLIENT_MAX_BACKFILL_MESSAGES` 50, `MAX_ACTIVE_PEERS` 4096, `MAX_CONCURRENT_BACKFILLS` 32, `MAX_FOLLOW_UP_ROUNDS` 10 | networking/timeouts |
| src/blob_transfer.rs | `READ_TIMEOUT_SECS` 30 | transfer timeout |
| src/catalogue_client.rs | `FETCH_TIMEOUT` 30 s, `DEFAULT_PAGE_SIZE` | networking |
| src/catalogue_handler.rs | `CATALOGUE_HANDLER_TIMEOUT` 60 s | networking |
| src/catalogue_rate_limits.rs | `MAX_CONCURRENT_CATALOGUE_CONNECTIONS` 16 | resource limits |
| src/abuse_controls.rs | `DEFAULT_MAX_DISPLAY_LENGTH` 10 000, `DEFAULT_MAX_SINGLE_LINE_LENGTH` 256 | message limits |
| main.rs | `LOG_QUEUE_CAPACITY` 8192, `LOG_ROTATED_FILES` 3 | log behaviour |
| app/discover.rs:139 | `DISCOVER_RECENTLY_SEEN_WINDOW` 24 h | recency *data* window (sort/eviction logic) |
| app/discover.rs:19–22 | `DISCOVER_MAX_NAME_CHARS` 64, `MAX_DESC_CHARS` 160, `MAX_TAG_CHARS` 24, `MAX_TAGS_SHOWN` 4 | character/count limits (not pixels) |
| shared_by_me_table.rs:408–418 | `MAX_VISIBLE_CHIPS` 3, `CHIP_LABEL_MAX_CHARS` 14, `KIND_MAX_CHARS` 12, `META_MAX_CHARS` 32 | truncation limits |
| video_file_card.rs:472 | `HEADER_FILENAME_MAX_CHARS` 56 | truncation limit |
| download_progress_view.rs:95 | `FAILURE_DIAGNOSTICS_MAX_CHARS` 160 | truncation limit |
| app/home.rs:130, 134 | `PEOPLE_PEERS_MAX` 3, `PEOPLE_ACTIVITY_MAX` 4 | displayed-row counts |
| offscreen_status_card.rs:30 | `CAPTURE_DIR` | file path |

**Borderline — flagged, left out until verified:**
- `presentation.rs:15` — `MESSAGE_GROUP_WINDOW_MS = 5 * 60 * 1000`. It is a *presentation*
  grouping rule, but it is **time-based, not pixel-based**. Per the PDF ("when uncertain whether a
  value is visual or behavioural, leave it out until verified") it stays out of the live theme
  system.
- `ui_components.rs:1605` — `SIDEBAR_FADE_FRAMES` (animation frames) is presentational but not a
  size/colour; low priority, optional `MotionTokens` later.

---

## 5. Recommended BoruTheme mapping (aligns with PDF Task 2)

```
BoruTheme {
  colors:      ColorTokens    ← design_tokens.rs accessors + §3.12 raw state/status colours
                               + app/dialogs.rs panel/backdrop + video/status-card literals
  typography:  TypographyTokens ← fonts.rs TypeRole (15 roles) + legacy sizes aliases;
                               absorb remaining raw .size(N) (chat chrome TYPO_*, calls, dialogs)
  spacing:     SpacingTokens  ← design_tokens.rs SPACE_* + remaining raw spacing()/padding()
  radii:       RadiusTokens   ← design_tokens.rs RADIUS_* + raw radii (pickers 8, dialogs 16,
                               media frame 13, avatar 12/48, settings 8/4)
  sidebar:     SidebarTheme   ← app/sidebar.rs paddings/pills/24px icons + SIDEBAR_* tokens
  home:        HomeTheme      ← app/home.rs PEERS_BODY_MIN/ACTIVITY_ROW_HEIGHT/card_gap +
                               quick_actions.rs + status_card.rs gaps/dims
  chat:        ChatTheme      ← app/chat.rs spinner/picker/screen-share dims + composer +
                               TYPO_* aliases + presentation.rs bubble width
  attachments: AttachmentTheme ← app/files.rs + shared_by_me_table.rs column widths +
                               download_progress_view.rs + video_file_card.rs + app.rs:718-720
  rooms:       RoomTheme      ← app/discover.rs row/banner/progress + catalogue rows
  tunnels:     TunnelTheme    ← app/tunnels.rs chip padding
  dialogs:     DialogTheme    ← app/dialogs.rs + boru_dialog.rs (mostly tokenized) +
                               app/chat.rs pickers
}
```

## 6. Migration notes

1. **Duplicates to consolidate first**: `IMAGE_PREVIEW_MAX_WIDTH/HEIGHT` exist in both
   `design_tokens.rs` and `app.rs:718-719`; `status_card.rs` gap constants overlap
   `design_tokens.rs` STATUS_CARD_* colour tokens.
2. **Legacy alias removal**: `app.rs:483` TYPO_* aliases and `fonts.rs` `sizes` module are
   migration scaffolding; the typed theme should keep only `TypeRole`.
3. **Colour math**: `app.rs:1231-1263` derives hover/pressed by multiplying RGB — replace with
   the existing `primary_hover`/`primary_pressed` accessors.
4. **Order of extraction** (per PDF Task 3): sidebar/shell → home → chat+bubbles → composer →
   file cards → video cards → room cards → friend cards → tunnel cards → dialogs/controls,
   compiling after each area.
5. **Guardrail**: no visual values changed in this task; appearance stays byte-for-byte the
   baseline (BORU-UI-02 introduces `BoruTheme::default()` that matches these exact values).
