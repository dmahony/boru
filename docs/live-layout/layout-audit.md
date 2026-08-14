# BORU-LAYOUT-01 — Layout Audit: separating structural layout from visual styling

Task: `t_12486b21` — first task of the Live Layout (TOML) chain
(`boru_live_layout_toml_tasks.pdf` Task 1). Purpose: **map only** — inventory the
structural-layout values that currently live in theme/design/view code and must move
into the new `LayoutConfig` model (`examples/iced_chat/layout.rs`), and explicitly
flag the values that must **stay** in `BoruTheme` (pure visual). No behaviour or
appearance changed; `layout.rs` is a skeleton not yet wired into views.

Audit scope: `examples/iced_chat/` (the Iced GUI). Line numbers are as of commit
`01fd3d4e` (origin/main, BORU-UI-23).

---

## 1. Summary

BoruTheme (`theme.rs`) currently mixes **pure-visual tokens** (colours, typography
sizes, radii, icon/avatar sizes, border widths, motion counts) with **structural
geometry** (row heights, max widths, gaps, paddings, table column widths, picker
sizes, breakpoints, section order/visibility). The BORU-UI chain intentionally left
the structural/geometry values in place; this chain extracts them.

New model: `examples/iced_chat/layout.rs` — `LayoutConfig` root with groups:

| Layout group | LayoutConfig struct | Structural categories |
|---|---|---|
| Home | `HomeLayout` | section order/visibility, grid/list mode, grid column portions, stack breakpoint, quick-action column counts, max content width, padding, gaps, card sizing |
| Sidebar | `SidebarLayout` | width/min/max/inset, section order/visibility, padding regions, row heights |
| Chat | `ChatLayout` | bubble/message widths, image preview caps, context menu, details panel, pickers, screen-share box, composer button placement, member list |
| Component | `ComponentLayout` | thumbnail position, metadata alignment, button placement, card orientation, video card sizing |
| Tables | `TablesLayout` | file-dashboard + sharing-table column widths |
| Responsive | `ResponsiveLayout` | viewport tiers, home content-width breakpoints |
| Future screens | `LayoutConfig::screens` | extension point (`BTreeMap<String, ScreenLayout>`) |

Every leaf `Default` reproduces the current appearance (the guardrail "layout
defaults must reproduce the current appearance when the config file is absent").

---

## 2. Classification rule (structural vs visual)

- **Structural (→ LayoutConfig):** anything that controls *arrangement, geometry,
  sizing or placement* — section order/visibility, grid/list mode, column counts,
  widths, heights, paddings, gaps, spacing, breakpoints, thumbnail position,
  metadata alignment, button placement, card orientation. These change *where* and
  *how big* elements are.
- **Pure visual (→ stays in BoruTheme):** anything that controls *appearance of a
  fixed-size element* — colours, typography sizes/weights/families, corner radii,
  border widths, icon/avatar sizes, glyph sizes, motion/animation counts,
  opacity/shadow values.
- **Behavioural (→ stays out of the layout system entirely):** timing windows,
  protocol/network constants, state flags that toggle features rather than layout.

When a value is ambiguous, the PDF guardrail says **leave it out until verified**;
the borderline entries below are flagged explicitly.

---

## 3. Structural values that move to LayoutConfig

### 3.1 Home dashboard (`app/home.rs`, `design_tokens.rs`, `theme.rs::HomeTheme`)

| Location | Role | LayoutConfig token |
|---|---|---|
| `app/home.rs:1499-1506` — left column push order (hero, mesh, actions) | section order | `home.section_order` (`Hero, MeshHealth, QuickActions, PeopleActivity, Tunnels`) |
| `app/home.rs:1443-1448` — right rail (people & activity, tunnels) | section order (rail) | `home.section_order` tail |
| `app/home.rs:1495-1496` — `rail_stacked = content_width < HOME_TWO_COL_CONTENT` | grid/list switch breakpoint | `home.grid.stack_breakpoint` + `home.mode` |
| `app/home.rs:1526-1541` — `FillPortion(2)` main / `FillPortion(1)` rail, `SPACE_24` gap | grid column portions | `home.grid.main_portion`, `rail_portion`, `column_gap` |
| `app/home.rs:1565-1569` — `h_padding = SPACE_32` large / `SPACE_28` else | horizontal padding | `home.padding.horizontal_large/default` |
| `app/home.rs:1594` — `Padding::from([SPACE_28, h_padding]).bottom(SPACE_32)` | canvas padding | `home.padding.top/bottom/horizontal_*` |
| `app/home.rs:1596` — `max_width(DASHBOARD_MAX_WIDTH)` | max content width | `home.max_content_width` |
| `app/home.rs:1576` — header→dashboard gap `SPACE_28 + SPACE_12` | gap | `home.gaps.header_dashboard_gap` |
| `app/home.rs:1578` — dashboard→footer gap `SPACE_16` | gap | `home.gaps.footer_gap` |
| `app/home.rs:1466` — compact header stack gap `SPACE_12` | gap | `home.gaps.compact_header_stack_gap` |
| `app/home.rs:1501-1513` — `card_gap = btheme.home.quick_action_gap` (20) | card gap | `home.gaps.card_gap` |
| `app/home.rs:810` — hero→mesh gap `btheme.home.hero_gap` (40) | gap | `home.gaps.hero_gap` |
| `app/home.rs:496/677` — `peers_body_min` (128) | card sizing | `home.card_sizing.peers_body_min` |
| `app/home.rs:586/836` — `activity_row_height` (32) | card sizing | `home.card_sizing.activity_row_height` |
| `quick_actions.rs:247-255` — `grid_columns_for`: 4 / 2 / 1 by content width | column counts | `home.quick_actions.columns_wide/mid/narrow` |
| `design_tokens.rs:325-328` — `HOME_QUICK_ONE_COL_CONTENT` 520 / `HOME_QUICK_FOUR_COL_CONTENT` 1000 | column breakpoints | `home.quick_actions.two_col_breakpoint/four_col_breakpoint` |
| `theme.rs:1274` — `quick_action_icon_size` (40) | card sizing | `home.card_sizing.quick_action_icon_size` |
| `status_card.rs:54-93` — `STATUS_CARD_MIN_CONTENT_HEIGHT`, `STATUS_CARD_MEDIUM/NARROW/MESH_HIDE_CONTENT`, `STATUS_CARD_TEXT_MIN_WIDTH`, `STATUS_CARD_MESH_MAX_WIDTH`, `STATUS_CARD_PADDING_X` | card sizing / breakpoints | `home.card_sizing.status_card_*` |
| `theme.rs:1281-1298` — `status_card_text_min_width_medium`, `status_card_mesh_max_width`, `status_card_padding_x`, `status_icon_text_gap_*`, `status_text_graph_gap_*`, `status_divider_width/height` | card sizing | `home.card_sizing.status_*` |

### 3.2 Sidebar (`app/sidebar.rs`, `design_tokens.rs`, `theme.rs::SidebarTheme`)

| Location | Role | LayoutConfig token |
|---|---|---|
| `design_tokens.rs:193-197` — `SIDEBAR_WIDTH` 304 / `SIDEBAR_WIDTH_MIN` 288 / `SIDEBAR_WIDTH_MAX` 320 / `SIDEBAR_INSET` 24 | width + inset | `sidebar.width`, `width_min`, `width_max`, `inset` |
| `app/sidebar.rs:290-382` — section push order (CHATS, GROUPS, FRIENDS, DISCOVER, PUBLIC ROOMS, REQUESTS) | section order | `sidebar.section_order` |
| `app/sidebar.rs:283-288` — collapsed/visibility gating per section index | section visibility | `sidebar.hidden_sections` (default empty = all visible) |
| `theme.rs:1210-1231` (`SidebarPadding`) — brand/identity/section/utility/join paddings (SPACE_16/8/4/8/4/8/12/12/8/4) | padding regions | `sidebar.padding.*` |
| `card_shell.rs:44/53` — `CARD_ROW_HEIGHT` 48, `PEER_ROW_HEIGHT` 60 | row heights | `sidebar.row_heights.conversation_row/peer_row` |
| `design_tokens.rs:237` — `PEER_PANEL_MAX_HEIGHT` 320 | panel max height | `sidebar.row_heights.peer_panel_max_height` |
| `card_shell.rs:59` — `DEFAULT_LIST_MAX_HEIGHT` 180 | list max height | `sidebar.row_heights.default_list_max_height` |

### 3.3 Chat (`app/chat.rs`, `design_tokens.rs`, `presentation.rs`, `theme.rs::ChatTheme`)

| Location | Role | LayoutConfig token |
|---|---|---|
| `design_tokens.rs:199-206` — `MESSAGE_MAX_WIDTH` 480, `CHAT_BUBBLE_MAX_WIDTH` 560, `CHAT_BUBBLE_WIDTH_RATIO` 0.68, `IMAGE_PREVIEW_MAX_WIDTH` 360, `IMAGE_PREVIEW_MAX_HEIGHT` 400 | bubble/message/image caps | `chat.bubble_max_width`, `bubble_width_ratio`, `message_max_width`, `image_preview_max_width/height` |
| `presentation.rs:29-33` — `chat_bubble_max_width` rule (560 or 68 % of timeline) | bubble sizing rule | `chat.bubble_*` (rule moves to view layer, values here) |
| `theme.rs:1343` — `context_menu_width` 180 | popover width | `chat.context_menu_width` |
| `design_tokens.rs:198` — `DETAILS_PANEL_WIDTH` 280 | details panel | `chat.details_panel_width` |
| `theme.rs:1345-1355` — emoji picker 280/160, GIF picker 320/300 + thumbs 150×100 | pickers | `chat.emoji_picker.*`, `chat.gif_picker.*` |
| `theme.rs:1357-1359` — screen-share 640×360 | viewer box | `chat.screen_share.*` |
| `app/chat.rs:3982` — composer row `attach, folder, input, gif, emoji, send`, `SPACE_6`, `SPACE_4` | composer button placement | `chat.composer.button_order`, `spacing`, `padding` |
| `app/chat.rs:1826-1832` — member list panel 300 wide / max 500, name `FillPortion(3)` role `FillPortion(1)` | member list | `chat.member_list.*` |

### 3.4 Component layout (`video_file_card.rs`, `theme.rs::VideoTokens`)

| Location | Role | LayoutConfig token |
|---|---|---|
| `video_file_card.rs:129-149` — `CardBand` narrow 560 / medium 780 breakpoints | media card bands | `component.video.narrow_breakpoint/medium_breakpoint` |
| `theme.rs:1473-1477` — `play_overlay_size` 64, `header_filename_max_width` 420, `controls_slider_width` 90 | media card sizing | `component.video.*` |
| PDF Task 5 defaults (not yet in code) | thumbnail position | `component.thumbnail_position` (Left) |
| PDF Task 5 defaults | metadata alignment | `component.metadata_alignment` (Start) |
| PDF Task 5 defaults | button placement | `component.button_placement` (Below) |
| PDF Task 5 defaults | card orientation | `component.card_orientation` (Horizontal) |

### 3.5 Data tables (`app/files.rs`, `shared_by_me_table.rs`, `theme.rs::AttachmentTheme`)

| Location | Role | LayoutConfig token |
|---|---|---|
| `theme.rs:1436-1452` (`FileTableColumns`) — 72/120/120/140/120/110/90/110/80/100/100/110 | file table column widths | `tables.file_table.*` |
| `app/files.rs:2661` — download Started 100, `files.rs:2622/2754` — State 100, `files.rs:3572` — Ago 110 | transfer row columns | `tables.file_table.download_started_col/download_state_col/activity_ago_col` |
| `theme.rs:1457-1463` (`SharedTableColumns`) — 144/64/122/80/36 | sharing table column widths | `tables.shared_table.*` |

### 3.6 Responsive breakpoints (`design_tokens.rs`, `theme.rs::ResponsiveTokens`)

| Location | Role | LayoutConfig token |
|---|---|---|
| `design_tokens.rs:210-215/240-245` — `VIEWPORT_REF/MIN/LG/XL` widths+heights | viewport tiers | `responsive.viewport_*` |
| `design_tokens.rs:248` — `CONTENT_MAX_WIDTH` 720 | generic content cap | `responsive.content_max_width` |
| `design_tokens.rs:333-345` — `HOME_ILLUSTRATION_FULL_CONTENT` 720, `HOME_ILLUSTRATION_HIDE_CONTENT` 520, `HOME_COMPACT_HEADER_CONTENT` 560 | home breakpoints | `responsive.home_illustration_*`, `home_compact_header_content` |
| `design_tokens.rs:256-282` — `sidebar_width_for`, `is_compact/is_medium/is_large` | derived tiers (functions of viewport tokens) | view layer derives from `responsive.viewport_*` |

---

## 4. Values that STAY in BoruTheme (pure visual — do NOT move)

| Group | Fields | Why it stays |
|---|---|---|
| `ColorTokens` (theme.rs:56-211) | all colours, alpha tints, backdrops, shadows | colour is pure appearance |
| `TypographyTokens` (theme.rs:639-890) | font sizes, weights, families, line heights | text appearance, not arrangement |
| `RadiusTokens` (theme.rs:935-991) | all corner radii (card, pill, dialog, media…) | corner shape, not layout |
| `IconTokens` (theme.rs:996-1022) | icon sizes xs..xl | glyph sizing |
| `AvatarTokens` (theme.rs:1027-1059) | avatar sizes sm/md/lg/profile/chat_list/chat_header/msg, status dots | glyph sizing |
| `BorderTokens` (theme.rs:1095-1118) | hairline/focus/tab_active/selected_row/media_frame widths | stroke width |
| `MotionTokens` (theme.rs:1168-1179) | sidebar_fade_frames | animation count, not geometry |
| `SidebarTheme` radii/text sizes (theme.rs:1194-1202) | `item_radius`, `avatar_container_radius`, `utility_icon_size`, `name_size`, `section_label_size` | radii + text sizes are visual |
| `HomeTheme` text sizes (theme.rs:1275-1280) | `quick_action_title_size`, `quick_action_desc_size`, `quick_action_desc_line_height` | typography |
| `HomeTheme` divider/security radii (theme.rs:1300-1302) | `status_divider_radius`, `security_pill_radius` | corner radii |
| `HomeTheme::show_activity_feed` (theme.rs:1306) | feature toggle | **behavioural flag**, not layout — stays out of the layout system |
| `ChatTheme::spinner_size` (theme.rs:1341) | 40 px spinner glyph | glyph sizing |
| `AttachmentTheme` chip/avatar/typography fields (theme.rs:1403-1419) | `chip_avatar_size`, `chip_label_size`, `detail_label_width` (label text), `progress_bar_girth`, `progress_pct_label_width` | glyph/label appearance |
| `RoomTheme` (theme.rs:1531-1554) | `catalogue_row_height`, `overscan`, `banner_width`, `progress_length`, `progress_girth` | **pending review**: row height/overscan/banner width are structural but the public-rooms screen has no layout task in this chain yet — leave until a future task covers Discover |
| `DialogTheme` (theme.rs:1578-1613) | `avatar_size`, `avatar_glyph_size`, `title_size`, `body_size`, `spacing`, `padding`, `control_padding_*`, `control_spacing` | **pending review**: spacing/padding are structural, but dialogs are overlays sized by content; keep in theme until a task explicitly targets dialog layout |
| `CallTheme` (theme.rs:1618-1644) | `avatar_size`, `avatar_glyph_size*`, `pip_w`, `pip_h`, `controls_gap` | **pending review**: pip/controls geometry is structural but the calls screen is not in this chain's scope |
| `ControlTokens` (theme.rs:1649-1669) | `header_height`, `slider_width`, `color_picker_radius`, `color_picker_bar_radius` | **pending review**: slider width is structural; settings screen not yet in scope |

> **Pending-review rule:** values that are plausibly structural but live on screens
> not covered by the live-layout chain (Discover, Dialogs, Calls, Settings) stay in
> BoruTheme for now. When a future task targets those screens it can lift the
> structural subset into a new `LayoutConfig` group without changing this model —
> the `screens` extension point exists for exactly that.

---

## 5. Behavioural / protocol constants that stay OUT of the layout system

| Location | Constant | Why |
|---|---|---|
| `presentation.rs:20` | `MESSAGE_GROUP_WINDOW_MS` (5 min) | message-grouping **timing** — presentation behaviour, not geometry |
| `src/` protocol/network constants | e.g. `MAX_MESSAGE_SIZE`, relay ports, gossip intervals | networking behaviour — the layout system must never touch protocol behaviour |
| `perf_tracker.rs` | perf instrumentation | dev tooling, not layout |

---

## 6. Defaults must reproduce the current appearance

`layout.rs` leaf defaults mirror the audited values exactly (e.g.
`HomeLayout::default().max_content_width = DASHBOARD_MAX_WIDTH`, `SidebarLayout::default().width = SIDEBAR_WIDTH`,
`QuickActionsLayout::default().columns_wide = 4`). BORU-LAYOUT-02 adds unit tests
asserting each default against `design_tokens.rs` / `card_shell.rs` constants (the
theme.rs test-module pattern), so the two sources can never drift apart.

---

## 7. Follow-ups for later tasks

- **BORU-LAYOUT-02 (design layout schema):** finish typed structs + defaults +
  partial-override shape (Option leaves or merge layer) + default-value unit tests.
- **BORU-LAYOUT-03 (home layout wiring):** wire `home.*` into `app/home.rs` view;
  section order/visibility becomes data-driven; grid columns read from
  `home.grid`/`home.quick_actions`; breakpoints from `responsive`.
- **Component layout task:** wire `component.*` (thumbnail position, metadata
  alignment, button placement, card orientation) into media/file cards.
- **Layout config file + watcher:** `boru-layout.toml` parsing, merge, validation,
  live reload — mirror `theme_config.rs` / `theme_merge.rs` / `theme_watcher.rs`.
