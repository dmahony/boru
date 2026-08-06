# FS-02 — File Sharing Dashboard Design-System Specification

## CARD / STATUS

- Card: FS-02 — Translate the mockup into Boru design-system specifications
- Status: Specification complete; no product code changed
- Date: 2026-08-03
- Phase: A — Discovery
- Depends: FS-00 (baseline audit)
- Written for: downstream UI implementation agents

## SUMMARY

This document specifies every visual token, layout rule, component, state, and
responsive behaviour for the Boru File Sharing dashboard screen. It maps the
approved dashboard structure into the existing Boru Modern design system
(`design_tokens.rs`, `fonts.rs`, `ui_components.rs`, `card_shell.rs`,
`icon_system.rs`) so a downstream UI agent can implement the screen without
inventing spacing, hierarchy, or states.

The shared application shell (sidebar, header, overlays) remains intact per the
existing `Screen` → `main panel + optional details` composition. Only the new
file-sharing screen surface is specified here. No new pickers, file browsers,
transfer protocols, or persistence layers are introduced — the native OS `rfd`
picker remains the sole file-selection mechanism.

---

## 1. SCREEN ANATOMY

### 1.1 Screen route

A new `Screen::FileSharing` variant is added to the `Screen` enum in
`examples/iced_chat/app.rs` (near the existing `ChatList`, `Chat`,
`PeerCatalogue` etc. variants). Navigation to this screen is dispatched by
sidebar/home affordances, not described here.

### 1.2 Region map

```
┌──────────────────────────────────────────────────────────────────┐
│  SIDEBAR (280 px, intact)  │  MAIN PANEL (Fill, bg=cvs)         │
│                             │                                    │
│  (unchanged from            │  ┌─ REGION 1: Header ────────────┐ │
│   current shell)            │  │  Title + Subtitle │ Search    │ │
│                             │  │  [Open Downloads Folder]     │ │
│                             │  └───────────────────────────────┘ │
│                             │  ┌─ REGION 2: Tab bar ──────────┐ │
│                             │  │  Shared by Me | Downloading  │ │
│                             │  │  Downloaded | Shared w/ Me   │ │
│                             │  │  Activity Log                │ │
│                             │  └───────────────────────────────┘ │
│                             │                                    │
│                             │  ┌──────── 2/3 ────────┬── 1/3 ──┐ │
│                             │  │ REGION 3:           │REGION 4:│ │
│                             │  │ File Table          │Peers    │ │
│                             │  │ (scrollable)        │Download- │ │
│                             │  │                     │ing from  │ │
│                             │  │ ┌─────────────────┐ │Me panel  │ │
│                             │  │ │ row: icon,name  │ │(scroll-  │ │
│                             │  │ │   metadata,chips│ │able)    │ │
│                             │  │ │   actions       │ │        │ │
│                             │  │ └─────────────────┘ │         │ │
│                             │  │ ...                 │         │ │
│                             │  ├─────────────────────┤         │ │
│                             │  │ REGION 5:           │REGION 6:│ │
│                             │  │ Recent Activity     │Sharing  │ │
│                             │  │ (card below table)  │Summary  │ │
│                             │  │                     │card     │ │
│                             │  └─────────────────────┴─────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

### 1.3 Region descriptions

| Region | Name | Content | Scroll |
|--------|------|---------|--------|
| 1 | Header | Page title "File Sharing" (left), subtitle line (left), search input + "Open Downloads Folder" action button (right). | No |
| 2 | Tab bar | Five tabs: Shared by Me, Downloading, Downloaded, Shared with Me, Activity Log. Active tab has a primary-green underline (2px). Inactive tabs are muted text. | No — tabs scroll horizontally if the window is too narrow |
| 3 | File table | The primary content area for the active tab. For "Shared by Me": a row-per-file table with file-type icon, two-line name/MIME, metadata, peer chips, download counts, trailing action menu. For "Downloading": same layout but with progress bars. | Yes (vertical) |
| 4 | Peers panel | "Peers Downloading from Me" — a live list of peers currently fetching files. Each row: avatar/initial, peer name, progress bar, file name being fetched. Empty state: "No peers are downloading right now." | Yes (vertical, bounded max height) |
| 5 | Recent activity | A card below the file table showing the most recent 10–15 file-sharing activity events (shares, downloads, completions, errors). Same pattern as the Home "Recent Activity" card (`CardShell`). | Yes (bounded max height) |
| 6 | Sharing summary | A card showing aggregate sharing stats: total files shared, total data transferred, active peer count. | No |

### 1.4 The tab bar in detail

Five tabs, in strict LTR order:

1. **Shared by Me** — files I have made available to peers (my catalogue entries)
2. **Downloading** — files currently being fetched (in-progress downloads)
3. **Downloaded** — completed downloads, sorted by recency
4. **Shared with Me** — files peers have made available to me (their catalogues, combined view)
5. **Activity Log** — a chronological stream of all file-sharing events

**Tab visual treatment:**
- Inactive: `text_secondary(theme)`, 14px Source Sans 3 Regular, no underline, `SPACE_4` vertical padding
- Active: `text_primary(theme)`, 14px Source Sans 3 SemiBold, 2px `primary(theme)` bottom border (underline), `SPACE_4` vertical padding
- Hover: `primary(theme)` text colour, no underline until active
- Horizontal gap between tabs: `SPACE_16`
- Tab bar padding from page edges: `SPACE_24` (left/right), `SPACE_8` (top/bottom)
- A 1px `border_muted(theme)` separator runs the full width below the tab bar

---

## 2. TOKEN MAPPING TABLE

Every value in the specification is mapped to an existing or proposed token.
The table below records the mapping decision for each visual attribute.

### 2.1 Reused tokens (no change needed)

| Attribute | Existing token | Value | Source file |
|-----------|---------------|-------|-------------|
| Page background | `color_canvas(theme)` | #F7F9F8 (light) / #1A1A2E (dark) | design_tokens.rs:251 |
| Card/surface background | `surface(theme)` | #FFFFFF (light) / #29293C (dark) | design_tokens.rs:269 |
| Primary text | `text_primary(theme)` | #17211B (light) / #CCC (dark) | design_tokens.rs:318 |
| Secondary text | `text_secondary(theme)` | #5F6F66 (light) / #999 (dark) | design_tokens.rs:327 |
| Muted text | `text_muted(theme)` | #64706A (light) / #999 (dark) | design_tokens.rs:420 |
| Primary accent | `primary(theme)` | #187F50 (light) / #4A9EFF (dark) | design_tokens.rs:429 |
| Primary hover | `primary_hover(theme)` | #147643 (light) / #5CB3FF (dark) | design_tokens.rs:354 |
| Primary soft bg | `primary_soft(theme)` | #EAF5EE (light) / rgba(0.15,0.3,0.15,0.4) (dark) | design_tokens.rs:376 |
| Success/online | `color_success(theme)` | #20A661 (light) / #3DDC84 (dark) | design_tokens.rs:385 |
| Danger/error | `color_danger(theme)` | #C84E4E (light) / #E64040 (dark) | design_tokens.rs:394 |
| Focus ring | `color_focus(theme)` | #2B9B67 (light) / #66CC66 (dark) | design_tokens.rs:403 |
| Warning | `color_warning(theme)` | #704505 (light) / #F2A626 (dark) | design_tokens.rs:412 |
| Standard border | `border_muted(theme)` | #DCE5DF (light) / #383852 (dark) | design_tokens.rs:300 |
| Strong border | `border_strong(theme)` | #C8D7CE (light) / #474766 (dark) | design_tokens.rs:309 |
| Hover background | `surface_hover(theme)` | #EFF3F1 (light) / #33334D (dark) | design_tokens.rs:287 |
| Selected background | `surface_selected(theme)` | #EDF7F1 (light) / #293A5A (dark) | design_tokens.rs:278 |
| Input background | `bg_input(theme)` | #F0F0F4 (light) / #222238 (dark) | design_tokens.rs:421 |

**Spacing tokens (all reused):**

| Token | Value | Used for |
|-------|-------|----------|
| `SPACE_4` | 4 px | Tab bar vertical padding, chip internal padding |
| `SPACE_8` | 8 px | Card internal spacing, row gaps, icon-to-text gap |
| `SPACE_12` | 12 px | Table row horizontal padding, header vertical padding |
| `SPACE_16` | 16 px | Card padding, column gap, page-level gap |
| `SPACE_20` | 20 px | Grid gap between table and side panel |
| `SPACE_24` | 24 px | Page horizontal padding, header spacing |
| `SPACE_32` | 32 px | Section vertical separation |

**Radius tokens (all reused):**

| Token | Value | Used for |
|-------|-------|----------|
| `RADIUS_SM` | 8 px | Buttons, search input, progress bar track |
| `RADIUS_MD` | 10 px | Table rows, peer chips |
| `RADIUS_LG` | 12 px | Cards, tab content panels |
| `RADIUS_XL` | 16 px | (not used on this screen) |

**Typography tokens (all reused from `fonts.rs`):**

| Token | Size/Weight | Used for |
|-------|------------|----------|
| `Typography::PageTitle` | 28 px SemiBold | "File Sharing" page title |
| `Typography::SectionHeading` | 18 px SemiBold | Card/section titles |
| `Typography::Body` | 14 px Regular | Table cell text, metadata, button labels |
| `Typography::SecondaryText` | 12 px Regular | File sizes, peer names, timestamps, chip labels |
| `Typography::SidebarSectionLabel` | 12 px SemiBold | Tab labels (uppercase) |
| `Typography::Timestamp` | 12 px Regular | Activity timestamps |
| `Typography::TechnicalValue` | 12 px JetBrains Mono | File hashes, content IDs (abbreviated) |

**Layout tokens (all reused):**

| Token | Value | Used for |
|-------|-------|----------|
| `SIDEBAR_WIDTH` | 280–320 px | Intact sidebar |
| `VIEWPORT_REF_WIDTH` | 1280 px | Primary design target |
| `VIEWPORT_MIN_WIDTH` | 1024 px | Narrow breakpoint |
| `VIEWPORT_LG_WIDTH` | 1440 px | Wide breakpoint |
| `CONTROL_HEIGHT` | 40 px | Search input, action buttons |
| `CONTROL_HEIGHT_COMPACT` | 36 px | Tab bar row height |

**Style helpers (all reused):**

| Helper | Used for |
|--------|----------|
| `surface_style(theme)` | Card backgrounds with border |
| `card_style(theme)` | Cards needing subtle shadow |
| `elevated_style(theme)` | Dropdown/context menus |
| `focus_border(theme)` | Focus ring on interactive elements |
| `shadow_card(theme)` | Card elevation |

**Existing UI component builders (all reused):**

| Component | Used for |
|-----------|----------|
| `Card` (ui_components.rs) | File table container, summary card |
| `CardShell` (card_shell.rs) | Recent activity, peers panel |
| `section_header()` | "Peers Downloading from Me" header |
| `divider()` | Section separators |
| `status_dot()` | Online/offline peer indicators |
| `badge()` | Download count badges, status pills |
| `empty_state()` | Empty table/panel states |
| `ghost_icon_button()` | Row action buttons (more menu, cancel) |
| `primary_button()` | "Open Downloads Folder" action |
| `secondary_button()` | Tab bar tabs (as button-group members) |
| `text_input_field()` | Search input |
| `Avatar` | Peer avatars in side panel |
| `card_header()` | Card title rows |

### 2.2 Extended tokens (new, added to design_tokens.rs)

These tokens do not exist yet but are semantic additions to the central token
module. They are NOT screen-specific — any future screen needing a progress bar
or file-type icon will use the same token.

| Token name | Type | Value | Rationale |
|-----------|------|-------|-----------|
| `PROGRESS_BAR_HEIGHT` | `f32` | `4.0` | Thin progress bar, readable but unobtrusive. Paired with percentage text. |
| `PROGRESS_BAR_HEIGHT_BOLD` | `f32` | `6.0` | Slightly thicker variant for download cards where the bar is the primary visual. |
| `TABLE_ROW_HEIGHT` | `f32` | `56.0` | Standard file-table row. Tall enough for two-line name/MIME + metadata. |
| `TABLE_ROW_HEIGHT_COMPACT` | `f32` | `48.0` | Compact row for Activity Log (single-line entries). Matches `CARD_ROW_HEIGHT`. |
| `CHIP_HEIGHT` | `f32` | `28.0` | Standard peer/status chip height. |
| `PEER_PANEL_MAX_HEIGHT` | `f32` | `320.0` | Bounded max height for the Peers panel before vertical scroll. |

**New colour token (one addition):**

| Token name | Light value | Dark value | Rationale |
|-----------|------------|------------|-----------|
| `color_progress_track(theme)` | `border_muted(theme)` (reuse) | same | Progress bar unfilled track — reuses the existing border token since it's visually identical. No new hex needed. |
| `color_progress_fill(theme)` | `primary(theme)` (reuse) | same | Progress bar filled portion — reuse primary accent. |

**Progress bar colours:** Both track and fill reuse existing tokens. The track
is `border_muted(theme)`; the fill is `primary(theme)`. No new colour constants
are required for progress bars.

### 2.3 No new one-off colours

All colour values on this screen come from the existing palette. The spec
explicitly prohibits `Color::from_rgb()` or `Color::from_rgba()` calls in the
file-sharing view code. Every colour is routed through a `design_tokens`
function.

---

## 3. COMPONENT INVENTORY AND INTERACTION-STATE MATRIX

### 3.1 Component catalogue for this screen

| # | Component | Builder / source | New or reuse |
|---|-----------|-----------------|--------------|
| C1 | Page header | Custom Row composition | New — but uses existing tokens and typography |
| C2 | Tab bar | Custom Row of button-group items | New component |
| C3 | File table row | New `file_table_row()` builder in ui_components.rs | New component |
| C4 | Peer chip | `badge()` variant or thin wrapper | Reuse `badge()` with peer-name content |
| C5 | File-type icon | `Icon` enum + size | Reuse `Icon::Files`, `Icon::Image`, `Icon::Play` etc. |
| C6 | Progress bar | New `progress_bar()` builder in ui_components.rs | New component |
| C7 | Action menu (trailing) | `ghost_icon_button()` wrapping `Icon::MoreVertical` | Reuse existing primitives |
| C8 | Search input | `text_input_field()` | Reuse |
| C9 | Peers panel | `CardShell` with peer rows | Reuse `CardShell` |
| C10 | Recent activity panel | `CardShell` with activity rows | Reuse existing Home pattern |
| C11 | Sharing summary card | `Card` builder | Reuse |
| C12 | Empty state | `empty_state()` | Reuse |
| C13 | Primary action button | `primary_button()` | Reuse |
| C14 | Status pill | `badge()` with coloured variant | Reuse |

### 3.2 Component interaction-state matrix

Every interactive component covers the full state set mandated by DESIGN_SYSTEM.md section 9:
Normal, Hover, Pressed, Selected, Keyboard Focused, Disabled, Error.

#### C1 — Page header

| State | Treatment | Notes |
|-------|-----------|-------|
| Normal | Title in `Typography::PageTitle` + `text_primary(theme)`, subtitle in `Typography::Body` + `text_secondary(theme)` | Static display |
| All other states | N/A — the header is not interactive | |

#### C2 — Tab bar (tab items)

| State | Treatment | Trigger |
|-------|-----------|---------|
| Normal (inactive) | `text_secondary(theme)`, 14px Regular, no underline | Default |
| Hover (inactive) | `primary(theme)` text colour, no underline | Mouse over |
| Pressed (inactive) | `primary_pressed(theme)` text colour | Mouse down |
| Active (selected) | `text_primary(theme)`, 14px SemiBold, 2px `primary(theme)` bottom border | Click / keyboard select |
| Keyboard focused | 2px `color_focus(theme)` outline ring around the tab item | Tab/Shift+Tab |
| Disabled | 40% opacity, no hover/press response | N/A for tabs (all always active) |

#### C3 — File table row

| State | Treatment | Trigger |
|-------|-----------|---------|
| Normal | `surface(theme)` background, `text_primary(theme)` name, `text_secondary(theme)` metadata | Default |
| Hover | `surface_hover(theme)` background, cursor change | Mouse over |
| Pressed | `surface_selected(theme)` background | Mouse down on row |
| Selected | `surface_selected(theme)` background + left-edge 3px `primary(theme)` stripe | Click / keyboard |
| Keyboard focused | 2px `color_focus(theme)` inset ring | Tab navigation |
| Disabled | 40% opacity on all text, no hover/press | N/A — rows are always interactive |
| Error | `color_danger(theme)` tint on the file name text + warning icon | Transfer failure, verification failure |

#### C4 — Peer chip

| State | Treatment | Trigger |
|-------|-----------|---------|
| Normal | `badge()` with `surface(theme)` background, `border_muted(theme)` 1px border, `text_primary(theme)` text | Default |
| Hover | `surface_hover(theme)` background, `primary(theme)` border | Mouse over |
| Pressed | `surface_selected(theme)` background | Mouse down |
| Keyboard focused | 2px `color_focus(theme)` outline | Tab/Shift+Tab |
| Disabled | 40% opacity | N/A |

#### C6 — Progress bar

| State | Treatment | Trigger |
|-------|-----------|---------|
| Normal | Track: `border_muted(theme)`, Fill: `primary(theme)`, percentage text in `Typography::SecondaryText` + `text_secondary(theme)` | Default |
| Indeterminate | Animated shimmer/gradient across the track, no percentage | Unknown total size |
| Paused | Fill: `color_warning(theme)`, "Paused" label | User pause |
| Error | Fill: `color_danger(theme)`, "Failed" label | Transfer failure |
| Complete | Fill: `color_success(theme)`, checkmark icon | 100% done |
| Disabled | 40% opacity on entire bar | N/A |

#### C7 — Trailing action menu (MoreVertical)

| State | Treatment | Trigger |
|-------|-----------|---------|
| Normal | `Icon::MoreVertical` at `IconSize::Sm`, colour `text_secondary(theme)` | Default |
| Hover | `primary(theme)` icon colour, `surface_hover(theme)` background circle | Mouse over |
| Pressed | `primary_pressed(theme)` icon colour | Mouse down |
| Open (menu visible) | `primary(theme)` icon colour, `elevated_style(theme)` dropdown below/to-left | Click |
| Keyboard focused | 2px `color_focus(theme)` ring | Tab/Shift+Tab |
| Disabled | 40% opacity icon | Row disabled |

#### C12 — Empty state (per tab)

| Tab | Empty state message | Icon |
|-----|-------------------|------|
| Shared by Me | "You haven't shared any files yet. Use the Share button in chat or your profile to make files available." | `Icon::Upload` at `IconSize::Xl` |
| Downloading | "No active downloads." | `Icon::Files` at `IconSize::Xl` |
| Downloaded | "No completed downloads." | `Icon::Check` at `IconSize::Xl` |
| Shared with Me | "No files have been shared with you yet. When a friend shares files, they'll appear here." | `Icon::Share` at `IconSize::Xl` |
| Activity Log | "No activity yet." | `Icon::Activity` at `IconSize::Xl` |

Empty state layout per tab: centred in the region, icon above text, 48px icon,
`Typography::Body` + `text_secondary(theme)` for the message, `SPACE_16` gap
between icon and text, `SPACE_32` vertical padding.

#### C13 — Primary action button ("Open Downloads Folder")

| State | Treatment | Trigger |
|-------|-----------|---------|
| Normal | `primary(theme)` fill, white text, `Typography::Body` SemiBold, `RADIUS_SM` | Default |
| Hover | `primary_hover(theme)` fill | Mouse over |
| Pressed | `primary_pressed(theme)` fill | Mouse down |
| Keyboard focused | 2px `color_focus(theme)` ring | Tab/Shift+Tab |
| Disabled | `surface(theme)` fill, 40% opacity text | No downloads dir configured |

### 3.3 Loading state

While tab content is loading (fetching catalogues, computing availability):

- The file table region shows a skeleton: 3–5 placeholder rows, each with a
  pulsing grey rectangle for the icon, two grey rectangles for the name/MIME,
  and a grey rectangle for the metadata area. Same dimensions as real rows.
- The peers panel shows its `CardShell` header + "Loading…" text in
  `Typography::Body` + `text_muted(theme)`, centred.
- The recent activity and summary cards appear after data arrives (they are not
  skeletonised — they simply don't render until populated).

### 3.4 Offline / disconnected state

When the node has no active connections (mesh health = offline):

- The tab bar remains visible and interactive (users can still browse local
  download history and their own shared files catalogue).
- A banner appears below the tab bar: amber background (`color_warning(theme)`
  at 10% opacity), `Typography::Body` text in `color_warning(theme)`: "You are
  offline. Peer lists and transfer progress may be stale."
- The Peers panel shows an empty state: "Offline — peer information unavailable."

### 3.5 Error state (per row)

When a specific file transfer fails (hash mismatch, network error, peer
disconnected):

- Row text colour shifts to `color_danger(theme)`.
- A small `Icon::AlertTriangle` at `IconSize::Xs` appears next to the file name.
- The progress bar fill becomes `color_danger(theme)`.
- The trailing action menu gains a "Retry" option.
- The error does NOT affect neighbouring rows — it's a per-row state.

### 3.6 Destructive state

When the user triggers a destructive action (stop sharing, cancel download,
remove from list):

- A confirmation is NOT a modal per the product register guidance. Use an
  inline confirmation: the row's trailing actions area is replaced with
  "Cancel | Confirm" text buttons. "Confirm" uses `color_danger(theme)`.
- After confirmation, the row fades out (or is simply removed — Iced 0.14 has
  no animation API, so instant removal is acceptable).

---

## 4. RESPONSIVE LAYOUT RULES

### 4.1 Breakpoints

| Name | Min width | Max width | Layout behaviour |
|------|-----------|-----------|-----------------|
| Narrow | 1,024 px | 1,279 px | Two-column collapses to single-column stacking. File table full-width, panels below. |
| Reference | 1,280 px | 1,439 px | Two-column: ~2/3 file table left, ~1/3 panels right. Tab bar full-width. |
| Wide | 1,440 px | ∞ | Same two-column proportion. Wider columns, more metadata visible. |

### 4.2 Reference layout (1,280 px window)

```
Window width: 1,280 px
Sidebar:     288 px (via sidebar_width_for(1280))
Main panel:  992 px
  Page padding (horizontal): 24 px each side → 944 px content area

  Header: full 944 px width
    Title + subtitle: left-aligned
    Search input + button: right-aligned, 320 px search width

  Tab bar: full 944 px width
    5 tabs × ~100 px each + gaps

  Two-column content area: 944 px
    Left column (file table): 600 px (~63%)
    Gap: 20 px (SPACE_20)
    Right column (panels): 324 px (~34%)
```

### 4.3 Narrow layout (1,024 px window)

```
Window width: 1,024 px
Sidebar:     288 px (minimum)
Main panel:  736 px
  Page padding: 24 px → 688 px content

  Header: stacks vertically
    Title + subtitle on top
    Search + button below (full width)

  Tab bar: full 688 px, tabs may scroll horizontally

  Single-column:
    File table: full 688 px width
    Peers panel below table: full 688 px width
    Recent activity below peers
    Summary card below activity
```

### 4.4 Wide layout (1,440 px+)

```
Window width: 1,440 px
Sidebar:     304 px (cap)
Main panel:  1,136 px
  Page padding: 32 px → 1,072 px content

  Left column (file table): 680 px (~63%)
  Gap: 24 px
  Right column (panels): 368 px (~34%)

  Additional metadata becomes visible in wider table rows
  (file size, MIME type, hash prefix).
```

### 4.5 Column proportions

The 63/34 split is deliberate: the file table is the primary surface and needs
room for file names, metadata, peer chips, and action menus. The side panel is
supporting information.

The proportion is achieved with `Length::FillPortion`:
- Left: `FillPortion(63)`
- Right: `FillPortion(34)`

At narrow, the single column uses `Length::Fill`.

### 4.6 Scroll behaviour

| Region | Scroll | Max height | Notes |
|--------|--------|------------|-------|
| File table | Vertical | None (grows to fill available space) | Scrollable when content exceeds viewport |
| Peers panel | Vertical | `PEER_PANEL_MAX_HEIGHT` (320 px) | Scrollbar appears when more than ~6 peers are active |
| Recent activity | Vertical | 200 px (matches `DEFAULT_LIST_MAX_HEIGHT`) | Same pattern as Home activity card |
| Summary card | None | Content-fitted | Static height |
| Tab bar | Horizontal (narrow only) | N/A | Only when tabs overflow the viewport |

---

## 5. FILE TABLE ROW SPECIFICATION

### 5.1 Row anatomy

```
┌─────────────────────────────────────────────────────────────────────────┐
│ [ICON]  Filename.pdf                              [PeerChip] [PeerChip] │
│  24px   application/pdf · 2.4 MB · shared 3h ago        ↓12     [ ⋮ ] │
│         ── 56 px row height ──                          downloads       │
└─────────────────────────────────────────────────────────────────────────┘
```

### 5.2 Row elements (left to right)

| Element | Specification |
|---------|--------------|
| File-type icon | `Icon` at `IconSize::Md` (20px). Maps MIME type → icon: images → `Icon::Image`, video → `Icon::Play`, archives → `Icon::Files`, documents → `Icon::Files`, audio → `Icon::Play`, other → `Icon::Paperclip`. Colour: `text_secondary(theme)`. |
| File name | `Typography::Body` (14px Regular), `text_primary(theme)`. Truncated with ellipsis if longer than ~40 characters. |
| MIME + size + age | Single line below name: `Typography::SecondaryText` (12px), `text_secondary(theme)`. Format: "application/pdf · 2.4 MB · shared 3h ago". |
| Peer chips (Shared by Me tab) | One `badge()` per peer who has downloaded or is downloading this file. Shows peer display name (truncated to 12 chars). Max 3 visible chips; "+N more" overflow chip if >3. `SPACE_4` between chips. |
| Download count (Shared by Me tab) | `Typography::SecondaryText`, `text_secondary(theme)`. Format: "↓12". Positioned between chips and action menu. |
| Progress bar (Downloading tab) | Replaces the MIME/size/age line. `PROGRESS_BAR_HEIGHT` (4px), full row width minus icon and action menu. Fill: `primary(theme)`. Track: `border_muted(theme)`. Percentage label to the right of the bar. |
| Recipient chip (Downloading tab) | Shows "From: <peer name>" as a `badge()`. |
| Trailing action menu | `Icon::MoreVertical` at `IconSize::Sm`, `ghost_icon_button()` style. Opens a dropdown with context-appropriate actions. |

### 5.3 Action menu contents (per tab)

**Shared by Me tab:**
- Copy link / share info
- Stop sharing (destructive)
- View details (hash, size, permissions)

**Downloading tab:**
- Pause download
- Cancel download (destructive)
- View details

**Downloaded tab:**
- Open file
- Open containing folder
- Share (re-share to another peer)
- Remove from list
- View details

**Shared with Me tab:**
- Download (if not yet downloaded)
- View peer catalogue
- View details

**Activity Log tab:**
- No action menu — activity log entries are read-only.

### 5.4 Row dimensions

| Property | Value | Token |
|----------|-------|-------|
| Row height | 56 px | `TABLE_ROW_HEIGHT` |
| Horizontal padding | 12 px | `SPACE_12` |
| Icon left margin | 12 px from row edge | `SPACE_12` |
| Gap icon → text | 12 px | `SPACE_12` |
| Gap between text lines (name → metadata) | 4 px | `SPACE_4` |
| Gap metadata → icons/chips | 8 px | `SPACE_8` |
| Action menu right margin | 8 px from row edge | `SPACE_8` |

---

## 6. DESIGN DECISIONS

### 6.1 No in-app file browser

The native OS `rfd` picker (`rfd::AsyncFileDialog`) remains the sole mechanism
for selecting files to share. The "Open Downloads Folder" button in the header
delegates to the OS (opens the default file manager at the Boru downloads
directory). No directory tree widget, file preview pane, or in-app navigation
is introduced.

### 6.2 Tab bar: horizontal scroll vs. wrap

Tabs scroll horizontally on narrow viewports rather than wrapping to a second
row. This keeps the tab bar at a fixed height (36px + 2px underline) and
preserves the active-tab-underline affordance. A horizontal scroll eliminates
the ambiguity of multi-row tab bars where the active tab's underline position
is confusing.

### 6.3 Progress bars: thin + numeric

Progress bars are deliberately thin (4–6 px) because they are secondary
information on the Shared by Me tab and compact metadata on the Downloading
tab. A numeric percentage label always accompanies the bar — colour alone is
insufficient for accessibility.

### 6.4 Peer chips vs. text list

Peer chips are used instead of a text list because:
- They provide a larger click target (28 px height).
- They allow future expansion (click a chip → open peer profile).
- They are visually scannable — distinct from the file metadata around them.

### 6.5 Activity Log: read-only stream

The Activity Log tab is a chronological event stream, not a table. It reuses
the Home Recent Activity pattern (`CardShell` with 48 px rows, icon + message +
timestamp). No actions, no selection, no hover state beyond the row highlight.

### 6.6 CardShell reuse

The Peers panel and Recent Activity panel use the existing `CardShell` builder
from `card_shell.rs`. This guarantees:
- Consistent header (uppercase title, count badge, "View all" action).
- Consistent row rhythm (48 px `CARD_ROW_HEIGHT`).
- Consistent empty-state messaging.
- Consistent scrolling behaviour (bounded max height, scrollbar on overflow).

### 6.7 No drag-and-drop

Iced v0.14 lacks native drag-and-drop support (confirmed in DESIGN_SYSTEM.md
section 12, item 15). No file-drop zone, drag-to-share, or drag-to-reorder is
specified. If a future Iced release adds DnD, revisiting this screen is a
separate card.

---

## 7. EXCLUSIONS (what this spec does NOT cover)

- **Sidebar modifications.** The sidebar remains exactly as-is.
- **Chat screen modifications.** File-sharing UI in the chat composer
  (AttachPressed, file rendering in chat log) is unchanged.
- **Settings screen modifications.** Any file-sharing settings belong in the
  Settings screen, not here.
- **New picker behaviour.** The native `rfd` picker is unchanged.
- **New protocol or persistence.** Catalogue, authorization, descriptor,
  transfer, and storage layers are unchanged.
- **New MCP endpoints.** GUI diagnostics remain loopback-only.
- **Multi-select on table rows.** Single-row selection only per this spec.
- **Sorting, filtering beyond search.** Only the search input is specified.
  Column-header sorting is a future enhancement.

---

## 8. VERIFICATION CHECKLIST

For the downstream UI implementer:

- [ ] All regions (1–6) are rendered at reference (1,280 px), narrow (1,024 px), and wide (1,440 px).
- [ ] All five tabs switch content correctly without layout shift.
- [ ] Every interactive state from the matrix (section 3.2) is visually distinguishable.
- [ ] Zero `Color::from_rgb()` or `Color::from_rgba()` calls in the file-sharing view code.
- [ ] Zero hardcoded spacing values — all via `SPACE_*` constants.
- [ ] Progress bars always paired with numeric percentage.
- [ ] Empty state messages are specific to each tab.
- [ ] No in-app file browser widget or directory tree exists.
- [ ] Sidebar, chat, settings, and overlay behaviour is unchanged.
- [ ] Keyboard navigation works: Tab through tabs, table rows, action buttons.
- [ ] Focus rings are visible on all interactive elements.
- [ ] Tab bar scrolls horizontally at narrow widths; does not wrap.

---

## 9. KNOWN LIMITATIONS / FOLLOW-UPS

- **No column sorting.** Sort controls on table headers (name, date, size) are
  a future enhancement. Current spec: default sort order is recency (newest first).
- **No bulk actions.** Select-all, multi-delete, multi-stop-sharing are not
  specified. Single-row actions only.
- **No drag-and-drop.** Iced v0.14 limitation. Revisit when Iced supports DnD.
- **No animation for progress bars.** Iced has no CSS transition equivalent.
  Bar width changes are instant (snap to value).
- **Activity Log pagination.** Current spec loads the most recent 50 events.
  Infinite scroll or "Load more" is a future enhancement.
- **Search scope.** The search input searches the current tab's content only.
  Cross-tab search is a future enhancement.

---

*End of FS-02 specification.*
