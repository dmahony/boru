# Boru Home Screen Baseline — Component Map & Design Token Catalog

**Task:** t_a0b1f82f (BORU-HOME-01)  
**Date:** 2026-08-11  
**Branch:** main (post POLISH-01..05 batch)  
**Status:** Baseline capture — zero behaviour change

---

## 1. Home Screen Regions (Top → Bottom)

```
┌──────────────────────────────────────────────────────────────┐
│ LEFT SIDEBAR (288–320 px)   │  MAIN CONTENT AREA             │
│ ┌────────────────────────┐  │  ┌──────────────────────────┐  │
│ │ Logo + Wordmark        │  │  │ PAGE HEADER              │  │
│ │                        │  │  │ Good {time}, {name}      │  │
│ │ CHATS section           │  │  │ Welcome to Boru           │  │
│ │ GROUPS section          │  │  │ [Connected pill] [Dl Mgr] │  │
│ │ FRIENDS section         │  │  └──────────────────────────┘  │
│ │ DISCOVER section        │  │  ┌──────────────────────────┐  │
│ │ PUBLIC ROOMS section    │  │  │ CONNECTION STATUS CARD   │  │
│ │ REQUESTS section        │  │  │ (dark green gradient)    │  │
│ │                        │  │  │ [Status indicator + mesh] │  │
│ └────────────────────────┘  │  └──────────────────────────┘  │
│                              │  ┌──────────────────────────┐  │
│                              │  │ MESH HEALTH CARD         │  │
│                              │  │ [Status row + stat tiles  │  │
│                              │  │  lobby + recent events]   │  │
│                              │  └──────────────────────────┘  │
│                              │  ┌──────────────────────────┐  │
│                              │  │ QUICK ACTIONS (2×2 grid) │  │
│                              │  │ [Public Room] [Group Chat]│  │
│                              │  │ [Add Friend] [Share Files]│  │
│                              │  └──────────────────────────┘  │
│                              │  ┌ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┐  │
│                              │  │ RIGHT RAIL (wide only)     │  │
│                              │  │ ONLINE PEERS card          │  │
│                              │  │ RECENT ACTIVITY card       │  │
│                              │  │ TUNNELS card               │  │
│                              │  └ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┘  │
│                              │  ┌──────────────────────────┐  │
│                              │  │ CONNECTION FOOTER STRIP  │  │
│                              │  │ Healthy · N direct · ...  │  │
│                              │  └──────────────────────────┘  │
└──────────────────────────────────────────────────────────────┘
```

---

## 2. Component Inventory

### 2.1 Left Sidebar
- **File:** `examples/iced_chat/app/sidebar.rs` (2137 lines)
- **Data sources:** `ConversationStore`, `Friends`, discovered peers (mDNS/DHT), `pub_rooms` directory, pending invite tickets
- **Sections (6 collapsible):** CHATS, GROUPS, FRIENDS, DISCOVER, PUBLIC ROOMS, REQUESTS
- **Key messages dispatched:** `OpenConversation`, `OpenFriendChat`, `JoinPublicRoom`, `AcceptGroupInvite`, `OpenFriendRequests`, `ShowCreateGroupDialog`
- **Conditional states:** Loading (spinner), Empty ("No conversations"), Populated (list rows with unread badges)
- **Width:** 288–320 px (`SIDEBAR_WIDTH: 304 px`, responsive `clamp`)

### 2.2 Page Header
- **File:** `examples/iced_chat/app/home.rs` (inline in `view_chat_list_content`, lines ~840–1382)
- **Data sources:**
  - `dep.local_label` → display name
  - `dep.time_of_day_greeting` → "Good morning/afternoon/evening" (computed by `Self::time_of_day_greeting()` in app.rs)
  - `dep.mesh_health` + `dep.has_peer_connections` + `dep.sender_ready` → connection variant
- **Sub-components:**
  - **Greeting** (`DisplayHeading`): "Good {time}, {name}" — Inter Tight Bold 32px
  - **Welcome subtitle**: "Welcome to Boru" — Public Sans Regular at `HOME_SUBTITLE` (16px)
  - **Status pill**: icon + label (Starting/Connecting/Connected/Degraded/Offline), colored border
  - **Download Manager button**: outline button opening `DownloadManager`
- **Responsive:** Compact header stacks pill under greeting; wide header places pill inline right
- **Key messages:** `OpenDownloadManager`, `OpenConnectionDetails`

### 2.3 Connection Status Card (Hero)
- **Files:** `examples/iced_chat/status_card.rs` (1094 lines, dedicated module)
- **Called from:** `view_chat_list_content` line ~916
- **Data sources:** `StatusCardDependency` — variant, content_width, headline, show_retry, show_details, pulse_frame, animate_mesh, dimmed_mesh, home_menu_opacity
- **Visual:**
  - Dark green gradient background (`#10201C → #091714 → #06100E`)
  - Thin low-contrast green border, 22px radius
  - Three layout modes (responsive):
    - **Mode A** (≥760px content): three-row left indicator + heading + canvas mesh on right
    - **Mode B** (≥520px content): compact horizontal
    - **Mode C** (<520px content): stacked narrow
  - Native canvas peer-to-peer mesh with slow pulse (~6s cycle, disabled for reduced-motion)
  - Two-tone heading: "Boru" in accent green, rest near-white
  - "Secure · Decentralized · Private" pill
- **Height:** ~218–235 px Ready state; content-driven growth for wrapped text
- **Conditional states (5):** Starting, Connecting, Ready, Degraded, Offline — mapped via `home_connection_variant()`
- **Key messages:** `AppMessage::RetryConnection`, `AppMessage::OpenConnectionDetails`

### 2.4 Mesh Health Card
- **File:** `examples/iced_chat/app/home.rs` lines ~930–1256
- **Wrapper:** `CardShell` with title_case=false, subtitle "Current connection status"
- **Data sources:**
  - `dep.mesh_health` → status badge (Healthy/Degraded/Offline)
  - `dep.direct_peers`, `dep.relayed_peers`, `dep.neighbors_len` → stat tiles
  - `dep.sender_ready` → lobby state ("Lobby: connected" / "Lobby: connecting…")
  - `dep.connected_age_secs` → connection duration
  - `dep.mesh_events` (Vec<MeshEventRow>, newest 4) → recent events list
- **Sub-components:**
  - **Status row:** icon + label + detail (content-driven height)
  - **Stat tiles (3):** Neighbors / Direct / Relayed — each a centred column with emphasized value + muted label, on surface_hover background, RADIUS_SM
  - **Lobby + duration row:** icon + text
  - **Divider + "Recent events" header + event rows** (bounded log, 4 newest)
- **Conditional states:** Healthy (Success badge), Degraded (Warning), Offline (Danger)
- **Key messages:** `AppMessage::OpenConnectionDetails` (header action)

### 2.5 Quick Actions Grid
- **File:** `examples/iced_chat/quick_actions.rs` (499 lines)
- **Called from:** `view_chat_list_content` line ~1260
- **4 actions (2×2 grid):**
  1. **Create Public Room** (Icon::Chat) → `AppMessage::CreateNewRoom`
  2. **Create Group Chat** (Icon::Users) → `AppMessage::ShowCreateGroupDialog`
  3. **Add Friend** (Icon::UserPlus) → `AppMessage::OpenFriendRequests`
  4. **Share Files** (Icon::Upload) → `AppMessage::AttachPressed`
- **Card structure:** 40px light-green circular icon tile + title (Public Sans SemiBold 16px) + description (Public Sans Regular 14px, 1.45 line height) + subtle chevron indicator
- **Responsive grid:**
  - ≥1000px content: 4 columns
  - ≥720px content (default): 2 columns
  - <520px content: 1 column
- **Button style:** `BUTTON_CARD` — full-card hit target, content-driven height

### 2.6 Right Rail (Online Peers / Recent Activity / Tunnels)

All three cards are lazy-rendered with independent `PartialEq` dependencies.

#### 2.6.1 Online Peers Card
- **File:** `examples/iced_chat/app/home.rs` lines ~332–477
- **Selector:** `online_peers_card_data()` — reads `self.friends`, `peer_presence_map`, `friend_image_handles`
- **Wrapper:** `CardShell` with count badge "N/M", "View all" → `OpenFriendRequests`
- **Row layout:** avatar (36px, with online dot) + name (Body) + presence label (SupportingText, coloured)
- **Empty state:** friend icon + spec copy, PEERS_BODY_MIN height (128px)
- **Height:** content-driven, capped at 5 visible rows (PEERS_BODY_MAX), scrolls beyond
- **Key message:** `AppMessage::OpenConversation(pk)` on row click

#### 2.6.2 Recent Activity Card
- **File:** `examples/iced_chat/app/home.rs` lines ~480–571
- **Selector:** `recent_activity_card_data()` — reads `self.recent_activity` ring buffer
- **Wrapper:** `CardShell` with count badge, empty icon/message
- **Row layout:** activity icon (coloured by kind: Online→green, others→muted) + description (truncated to 75 chars) + relative timestamp
- **Row height:** 32px (compact/dense)
- **Max list height:** 180px then scrolls
- **Key messages:** none (read-only display)

#### 2.6.3 Tunnels Card
- **File:** `examples/iced_chat/app/home.rs` lines ~574–696
- **Selector:** `tunnels_card_data()` — reads `TunnelService::list_tunnels()`
- **Wrapper:** `CardShell` with count badge, header action "Create tunnel" (empty) / "View all" (populated)
- **Row layout:** lock icon (coloured by status) + name + endpoint (TechnicalValue) + status label + close button
- **Row height:** 48px (CARD_ROW_HEIGHT)
- **Max list height:** 120px then scrolls
- **Key messages:** `AppMessage::CloseTunnel(id)`, `AppMessage::ShowCreateTunnelDialog`

### 2.7 Connection Footer Strip
- **File:** `examples/iced_chat/app/home.rs` lines ~1440–1456 (inline, via `connection_footer()` function in app.rs)
- **Data sources:** health_label, health_color, direct_peers, relayed_peers, neighbors_len, encryption_label
- **Visual:** Compact strip below the main content, single row with health badge + counts
- **Conditional:** "QUIC encrypted" when peers exist, "Idle" otherwise

---

## 3. Layout Grid (Two-Column Dashboard)

- **File:** `examples/iced_chat/app/home.rs` lines ~1384–1438
- **Wide layout (content ≥720px):**
  - Left column: `FillPortion(9)` ≈ 64.3% — Hero card + Mesh Health + Quick Actions
  - Right column: `FillPortion(5)` ≈ 35.7% — Online Peers + Recent Activity + Tunnels
  - Column gap: 24px (`SPACE_24`)
  - Both columns height-shrink (no stretch)
  - `align_y(Alignment::Start)` — top-aligned
- **Narrow layout (content <720px):**
  - Rail stacks BELOW left column
- **Card vertical gaps:** 20px (`SPACE_20`)
- **Page header → cards gap:** 40px (`SPACE_28 + SPACE_12`, POLISH-05)
- **Horizontal padding:** 32px large / 28px normal
- **Dashboard max-width:** 1480px (`DASHBOARD_MAX_WIDTH`), centred
- **Content width formula:** `window_width - sidebar_divider - sidebar_width - 2 * h_padding`

---

## 4. Design Token Catalog

### 4.1 Color Palette (Theme-Aware)

| Token | Function | Light Value | Dark Value |
|-------|----------|-------------|------------|
| `color_canvas()` | Page background | `#F7F9F8` | near-black |
| `bg_surface()` | Card/surface bg | `#FFFFFF` | dark surface |
| `surface_hover()` | Interactive row hover | light green tint | dark green tint |
| `text_primary()` | Primary body text | `#17211B` | near-white |
| `text_secondary()` | Secondary text | `#5F6F66` | muted light |
| `text_muted()` | Muted/supporting text | `#64706A` | muted |
| `primary()` | Primary brand green | `#187F50` | adjusted green |
| `primary_hover()` | Primary hover | `#147643` | lighter green |
| `primary_soft()` | Soft green bg | `#EAF5EE` | dark green |
| `accent_green()` | Success/connected | `#1A7F48` | brighter |
| `color_warning()` | Warning/amber | `#704505` | amber |
| `color_error()` | Danger/error | `#C84E4E` | red |
| `border_muted()` | Card borders | `#E8F0EB` | dark border |
| `border_strong()` | Strong borders | `#C8D7CE` | lighter |
| `bg_hover()` | Generic hover bg | tinted | tinted |

### 4.2 Spacing Scale

| Token | Value | Use |
|-------|-------|-----|
| `SPACE_2` | 2px | Row spacing, stat tile padding |
| `SPACE_4` | 4px | Title→subtitle gap |
| `SPACE_6` | 6px | Icon→text gap, button padding |
| `SPACE_8` | 8px | Header gaps, element spacing |
| `SPACE_10` | 10px | Status pill V padding (POLISH-05) |
| `SPACE_12` | 12px | Card internal gaps |
| `SPACE_16` | 16px | Card header→body gap, quick action V-pad |
| `SPACE_18` | 18px | Chat message group gap |
| `SPACE_20` | 20px | Card-to-card vertical gaps |
| `SPACE_24` | 24px | Card internal padding, column gap |
| `SPACE_28` | 28px | Page top padding (normal) |
| `SPACE_32` | 32px | Page horizontal padding (large) |
| `SPACE_40` | 40px | Page header→dashboard gap (POLISH-05) |

### 4.3 Corner Radii

| Token | Value | Use |
|-------|-------|-----|
| `RADIUS_SM` | 8px | Small controls, stat tiles |
| `RADIUS_MD` | 10px | Buttons, list selections |
| `RADIUS_LG` | 12px | Chat bubbles, dialogs |
| `RADIUS_CARD` | 16px | Card containers (unified POLISH-03) |
| `RADIUS_XL` | 16px | Hero cards, composer |

### 4.4 Typography Roles (`fonts::TypeRole`)

| Role | Family | Weight | Size | Use |
|------|--------|--------|------|-----|
| `DisplayHeading` | Inter Tight | Bold 700 | 32px | Page greeting |
| `PageTitle` | Inter Tight | Bold 700 | 28px | App page title |
| `SectionTitle` | Public Sans | SemiBold 600 | 20px | Section heading |
| `CardTitle` | Public Sans | SemiBold 600 | 18px | Dashboard card title |
| `Body` | Public Sans | Regular 400 | 15px | Body copy |
| `BodyEmphasised` | Public Sans | SemiBold 600 | 15px | Emphasised body |
| `ButtonLabel` | Public Sans | SemiBold 600 | 14px | Buttons |
| `SupportingText` | Public Sans | Regular 400 | 13px | Secondary copy |
| `Metadata` | Public Sans | Regular 400 | 12px | Timestamps, counts |
| `ChatMessage` | Figtree | Regular 400 | 15px | Chat body |
| `ChatSender` | Figtree | SemiBold 600 | 14px | Sender name |
| `ChatMetadata` | Figtree | Regular 400 | 12px | Message timestamps |
| `ComposerText` | Figtree | Regular 400 | 15px | Input text |
| `TechnicalValue` | JetBrains Mono | Regular 400 | 12px | Hashes, ports |
| `BrandWordmark` | Raleway | ExtraBold 800 | 28px | BORU logo |

**Home-specific overrides:**
- `fonts::HOME_SUBTITLE` = 16px (welcome subtitle, uses Body role family)
- `QUICK_ACTION_TITLE_SIZE` = 16px (quick action card titles, uses CardTitle family)
- `QUICK_ACTION_DESCRIPTION_SIZE` = 14px (quick action descriptions)

### 4.5 Responsive Breakpoints (Content Width)

| Token | Value | Meaning |
|-------|-------|---------|
| `VIEWPORT_REF_WIDTH` | 1280px | Reference design target |
| `VIEWPORT_MIN_WIDTH` | 1024px | Minimum supported |
| `HOME_QUICK_FOUR_COL_CONTENT` | 1000px | 4-column quick actions |
| `HOME_TWO_COL_CONTENT` | 720px | Two-column dashboard active |
| `HOME_ILLUSTRATION_FULL_CONTENT` | 720px | Status card full layout |
| `HOME_COMPACT_HEADER_CONTENT` | 560px | Compact card headers |
| `HOME_ILLUSTRATION_HIDE_CONTENT` | 520px | Status card illustration hidden |
| `HOME_QUICK_ONE_COL_CONTENT` | 520px | 1-column quick actions |

### 4.6 Card Shell Geometry (`card_shell.rs`)

| Token | Value |
|-------|-------|
| `CARD_ROW_HEIGHT` | 48px (single-line rows) |
| `PEER_ROW_HEIGHT` | 60px (two-line Online Peers rows) |
| `DEFAULT_LIST_MAX_HEIGHT` | 180px (scrollable body cap) |
| Card internal padding | 24px all sides (`SPACE_24`) |
| Card border radius | `RADIUS_CARD` (16px) |

### 4.7 Layout Dimensions

| Token | Value |
|-------|-------|
| `SIDEBAR_WIDTH` | 304px (target, 288–320 range) |
| `DASHBOARD_MAX_WIDTH` | 1480px |
| `DETAILS_PANEL_WIDTH` | 280px |
| `AVATAR_SM` | 36px |

---

## 5. Key Callbacks & Navigation

| Component | User Action | Message Dispatched |
|-----------|------------|-------------------|
| Status pill (header) | Click status pill | (display only) |
| Download Manager btn | Click | `AppMessage::OpenDownloadManager` |
| Hero card | Click "Retry" | `AppMessage::RetryConnection` |
| Hero card | Click "View details" | `AppMessage::OpenConnectionDetails` |
| Mesh Health card | Click "View details" | `AppMessage::OpenConnectionDetails` |
| Quick Action: Public Room | Click card | `AppMessage::CreateNewRoom` |
| Quick Action: Group Chat | Click card | `AppMessage::ShowCreateGroupDialog` |
| Quick Action: Add Friend | Click card | `AppMessage::OpenFriendRequests` |
| Quick Action: Share Files | Click card | `AppMessage::AttachPressed` |
| Online Peers row | Click row | `AppMessage::OpenConversation(pk)` |
| Online Peers header | Click "View all" | `AppMessage::OpenFriendRequests` |
| Tunnels row | Click close (×) | `AppMessage::CloseTunnel(id)` |
| Tunnels header | Click action | `AppMessage::ShowCreateTunnelDialog` |

---

## 6. Conditional States Summary

### Connection Variant (`HomeConnectionVariant`)
Computed by `home_connection_variant(mesh_health, has_peer_connections, relay_reachable)`:

1. **Starting** — `sender_ready=false`, no peers, `main_screen_reconnect_frame` drives braille dots animation
2. **Connecting** — `sender_ready=false`, some peers → "Connecting — waiting for peers…"
3. **Ready** — `sender_ready=true` or has peers, mesh Good → green check, "Boru is connected and ready."
4. **Degraded** — mesh Degraded(reason) → warning icon, mesh reasons displayed
5. **Offline** — mesh Offline(reason) → red icon, retry/details actions shown

### Lazy Rebuild Triggers
Each card has an independent `PartialEq` dependency so only the changed card rebuilds:
- **Online Peers:** rebuilds on friend list change, presence change, profile image arrival
- **Recent Activity:** rebuilds on ring-buffer push + per-second `activity_tick` (relative timestamps)
- **Tunnels:** rebuilds on tunnel snapshot change + per-second `activity_tick` (expiry flips)
- **ChatList (whole screen):** rebuilds on any of: mesh health, sender readiness, peer counts, window width, dark mode, hero pulse frame, reduced motion, or any rail-card dependency change

---

## 7. File Map (Quick Reference)

| Region | Primary File | Lines | Notes |
|--------|-------------|-------|-------|
| Page header + greeting | `app/home.rs` | ~840–1382 | `view_chat_list_content` |
| Hero card | `status_card.rs` | 1–1094 | Dedicated module, theme-independent |
| Mesh Health card | `app/home.rs` | ~930–1256 | Inline in `view_chat_list_content` |
| Quick Actions grid | `quick_actions.rs` | 1–499 | 4 cards, 2×2 responsive grid |
| Online Peers card | `app/home.rs` | ~332–477 | `view_online_peers_card` |
| Recent Activity card | `app/home.rs` | ~480–571 | `view_recent_activity_card` |
| Tunnels card | `app/home.rs` | ~574–696 | `view_tunnels_card` |
| Footer strip | `app/home.rs` | ~1440–1456 | `connection_footer()` in app.rs |
| Layout grid | `app/home.rs` | ~1384–1498 | 9:5 FillPortion, responsive stack |
| Card shell (reusable) | `card_shell.rs` | 1–933 | Builder pattern |
| Sidebar (6 sections) | `app/sidebar.rs` | 1–2137 | Collapsible sections, lazy-rendered |
| Design tokens | `design_tokens.rs` | ~330 lines | Spacing, radii, colors, dimensions |
| Fonts/Typography | `fonts.rs` | 833 lines | TypeRole enum, families, weights |
| Icons | `icon_system.rs` | — | Icon enum, sizes, color_fn builder |
| UI components | `ui_components.rs` | — | gutter_scrollable, divider, badge |
| App shell (header/inner) | `app.rs` | ~17000+ | Top-level view, sidebar+main split |
