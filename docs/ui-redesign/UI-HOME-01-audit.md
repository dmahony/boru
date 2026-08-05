# UI-HOME-01 — Home Screen Architecture Audit + Baseline

- Task: `t_0cda9247` (UI-HOME-01)
- Plan source: `Boru_Home_Screen_Tidy_and_Fonts_Hermes_Kanban_Plan.pdf` (attached to parent task `t_302a8ab8`)
- Repo: `/home/dan/iroh-gossip-chat` @ `main` (`15bf4e7d` at audit time)
- Status: READ-ONLY audit. No production code modified.

## 1. What the home screen is

The Boru home screen is the **ChatList landing screen** (`Screen::ChatList`), shown when no
conversation is selected. It is not a separate module — it is a large static view built inside
`app.rs` plus one helper module (`quick_actions.rs`) and three shared primitives
(`card_shell.rs`, `ui_components.rs`, `design_tokens.rs`).

### Route / view chain

| Step | Location |
|---|---|
| Screen enum | `examples/iced_chat/app.rs:2473` (`Screen::ChatList` at `:2475`) |
| Top-level view routing | `app.rs:21879-21880` — `Screen::ChatList => self.view_main_empty_state()` |
| Landing view (lazy wrapper) | `app.rs:24649-24652` — `view_main_empty_state()` |
| Screen dependency snapshot | `app.rs:24655-24680` — `chat_list_dependency()` → `ChatListDependency` (`app.rs:3993-4010`) |
| Static screen renderer | `app.rs:24685-25187` — `view_chat_list_content()` (runs inside `iced::widget::lazy`) |
| Root layout (sidebar + divider + main panel) | `app.rs:21905-21937` |

The whole screen is memoized with `iced::widget::lazy` keyed on `ChatListDependency`
(`app.rs:24650-24651`), so it only rebuilds when one of its rendered slices changes.

## 2. Component tree (all render locations)

```
Screen::ChatList (app.rs:2475)
└─ view_main_empty_state (app.rs:24649) → lazy(view_chat_list_content)
   └─ gutter_scrollable (app.rs:25178)                    — vertical page scroll
      └─ page_header (app.rs:25091-25102)
         ├─ greeting text "Good {time}, {name}" (app.rs:24757-24770, PAGE_TITLE 28px)
         ├─ welcome_line "Welcome to Boru" (app.rs:24771-24774)
         └─ status_pill "Starting|Connecting|Connected|Degraded|Offline" (app.rs:24776-24797)
      └─ main_content (app.rs:25104-25139)
         ├─ wide (window ≥ 900): left_col FillPortion(2) + right_col FillPortion(1) (app.rs:25131-25138)
         │   left_col (app.rs:25123-25130):
         │   ├─ hero_card (app.rs:24799-24889)            — hero badge 48×48 + headline + Retry/Details
         │   │                                             + NETWORK_MOTIF svg 200×140 (wide only, app.rs:24863-24870)
         │   ├─ mesh_card "Mesh Activity" (app.rs:24969-25039) — status label + detail + "View details"
         │   └─ action_grid (app.rs:25041-25042)          — quick_actions::quick_action_grid
         │   right_col (app.rs:25082-25089):
         │   ├─ online_card    = lazy(online,  view_online_peers_card)   (app.rs:25076-25077)
         │   ├─ activity_card  = lazy(activity, view_recent_activity_card) (app.rs:25078-25079)
         │   └─ tunnels_card   = lazy(tunnels, view_tunnels_card)        (app.rs:25080)
         └─ narrow (< 900): hero → right rail → mesh → actions stacked (app.rs:25108-25120)
      └─ footer = connection_footer(...) (app.rs:25141-25157, ui_components.rs:53-103)
```

Rail card builders (all memoized per-card):

| Card | Builder | Location |
|---|---|---|
| Online Peers | `view_online_peers_card` | `app.rs:24441-24499` |
| Recent Activity | `view_recent_activity_card` | `app.rs:24502-24565` |
| Tunnels | `view_tunnels_card` | `app.rs:24568-24643` |
| Quick actions | `quick_action_card` / `quick_action_grid` | `quick_actions.rs:65-92` / `:163-181` |
| Card shell (shared rail chrome) | `CardShell` | `card_shell.rs:51-207` |

## 3. State sources (every visible value → live source)

| Visible value | State source |
|---|---|
| Greeting name | `self.local_label` (app.rs:24758-24762) |
| Time-of-day greeting | `self.time_of_day_greeting()` (app.rs:21679) |
| Hero/pill variant (Starting/Connecting/Ready/Degraded/Offline) | pure fn `home_connection_variant(mesh_health, has_peer_connections, relay_reachable)` (app.rs:1416-1434) fed by `self.mesh_health` (`MeshHealth`, `src/chat_core.rs:392`), `self.neighbors`/`self.relayed_peers`/`self.direct_peers`, `self.sender.is_some()` (app.rs:24656-24657, 24694-24698) |
| Reconnect animation dots | `self.main_screen_reconnect_frame` (SplashTick 100 ms, app.rs:24704-24708; subscription main.rs:1632-1640) |
| Mesh Activity status/detail | same variant + peer counts + `self.mesh_connected_at` (app.rs:24943-24967; maintained at app.rs:8094-8103) |
| Quick-action labels/descriptions | static `ACTIONS` const (quick_actions.rs:30-55) — dispatch real `AppMessage`s |
| Online Peers rows | `self.friends` filtered by `self.peer_presence(&pk) != Offline`, `self.resolve_name(&pk)`, `self.friend_image_handles` (app.rs:24343-24373) |
| Recent Activity rows | `self.recent_activity: VecDeque<RecentActivityEvent>` ring buffer (app.rs:3663, cap 50; pushed at app.rs:8121-8127; rendered `take(15)`) |
| Relative timestamps | `crate::presentation::relative_time_from_system` (app.rs:24517), refreshed by per-second `ActivityTick` (`self.activity_tick`) |
| Tunnels rows | `self.tunnel_service.list_tunnels()` + `self.shared_tunnels` name map + `self.names` (app.rs:24400-24437) |
| Footer counts / encryption | `health_label` + `direct_peers`/`relayed_peers`/`neighbors_len`; `"QUIC encrypted"` when peers > 0 else `"Idle"` (app.rs:25145-25157) |
| Window width (responsive) | `self.window_width` from `WindowResized` (app.rs:19044-19046; init 1200 at app.rs:7118) |
| Theme | `self.dark_mode` → `theme_from_dark` (app.rs:24690) |

All values are live — no static/mock content on the home screen (per plan constraint).

## 4. Clipping root cause — quick-action descriptions

### Exact code

- `examples/iced_chat/quick_actions.rs:88` — `.height(Length::Fixed(132.0))` on the quick-action **button**.
- `quick_actions.rs:87` — `.padding([SPACE_12, SPACE_16])` → 24 px vertical padding; inner content area = **108 px**.
- `quick_actions.rs:66-83` — content `Column` (spacing 0, centered):
  - `icon_tile(Icon::*, IconSize::Lg)` → 40 px tile (`ui_components.rs:319`: `tile_size = px + SPACE_16`; `icon_system.rs:219`: `Lg = 24.0`)
  - `Space` 8 px (`quick_actions.rs:68`)
  - label at `TYPO_MD` = 15 px (`fonts.rs:175`), Semibold (`quick_actions.rs:70-73`) → line 19.5 px (iced default `LineHeight::Relative(1.3)`)
  - `Space` 4 px (`quick_actions.rs:74`)
  - description at `TYPO_XS` = 12 px (`fonts.rs:179`), muted, `width(Fill)` (`quick_actions.rs:76-80`) → 15.6 px/line
- Text defaults (iced 0.14.0): `Wrapping::Word` default (`~/.cargo/.../iced_core-0.14.0/src/text.rs:181-196`), `LineHeight::Relative(1.3)` default (`text.rs:197-221`). Neither the label nor the description sets a wrap strategy.

### Why it clips

Vertical budget inside the fixed 132 px button:

| Content | One-line | Two-line |
|---|---|---|
| icon tile | 40 | 40 |
| spacer | 8 | 8 |
| label (15 px × 1.3) | 19.5 | 39 |
| spacer | 4 | 4 |
| description (12 px × 1.3) | 15.6 | 31.2 |
| **Total needed** | **87.1** | **122.2** |
| Available (132 − 24 padding) | **108** | **108** |

- One-line label + one-line description: fits (87.1 ≤ 108).
- Label or description wraps to two lines: still fits alone (102.7 ≤ 108).
- **Label AND description both wrap (2 lines each): needs 122.2 px > 108 px → the bottom description line is clipped.**

The wrap happens because the grid has **4 columns at window width ≥ 1040** (`quick_actions.rs:152-160`,
`grid_columns_for`), and at common widths each card is very narrow:

| Window | Sidebar | Main panel | Padding | Left col (2/3) | Card width | Text width (card − 32) |
|---|---|---|---|---|---|---|
| 1600 | ~304 | 1295 | 32 | ~807 | ~195 | ~163 |
| 1280 (reference) | 304 | 975 | 24 | ~604 | ~145 | ~113 |
| 1024 | 288 | 735 | 16 | ~455 (2 cols) | ~223 | ~191 |

At 1280 px, text width ≈ 113 px: the label "Create Public Room" (≈ 128 px at 15 px Semibold) wraps
to 2 lines and the description "Open a room for anyone to join" wraps to 2 lines → **clipped**.
At 1024 px the grid drops to 2 columns (`< 1040`), cards are ~223 px wide and mostly fit on one
line, so clipping is mild/borderline there. At 1600 px cards are ~195 px wide: labels fit on one
line, descriptions wrap to two — still inside budget, so little/no clipping.

**Root cause:** a hard-coded fixed card height (`quick_actions.rs:88`) that cannot grow with
word-wrapped content; iced buttons with `Length::Fixed` clip overflowing content instead of
expanding. This matches the plan's "fixed-height … fixes that merely conceal layout problems"
constraint — the correct fix (UI-HOME-06) is content-driven sizing (drop the fixed 132 px and let
the card size to content, keeping the internal spacing rhythm), or a saner 2×2 grid at ≥ 1040 px.

### Other fixed-size / truncation constraints on the home screen (inventory)

| Constraint | Location | Effect |
|---|---|---|
| Quick-action card height `Fixed(132)` | `quick_actions.rs:88` | **clips descriptions (root cause)** |
| Online Peers row height `Fixed(48)` (`CARD_ROW_HEIGHT`) | `app.rs:24472`; `card_shell.rs:32` | single-line rows |
| Online Peers list `max_height` 5×48 + 4×2 = 248 | `app.rs:24495-24497` | 6th peer scrolls |
| Recent Activity row height `Fixed(32)` | `app.rs:24512`, `:24553` | dense rows |
| Recent Activity `max_height(180)` | `app.rs:24563` | list scrolls |
| Recent Activity description truncated at 40 chars + `Wrapping::None` | `app.rs:24540-24546` | long events get ellipsis |
| Tunnels row height `Fixed(48)` | `app.rs:24630` | single-line rows |
| Tunnels `max_height(120)` | `app.rs:24641` | list scrolls |
| `CardShell` default list max height 180 | `card_shell.rs:38`, `:189` | shared rail bound |
| Hero badge 48×48, NETWORK_MOTIF svg 200×140 | `app.rs:24806-24807`, `:24865-24868` | fixed decorations |
| Connection-details dialog 680×540 max | `connection_details.rs:21-22` | modal bound |

The page itself is NOT height-capped: the whole screen is wrapped in `gutter_scrollable`
(`app.rs:25178`; `ui_components.rs:1497-1505`), so vertical page growth scrolls. No fixed page
height, no hidden-overflow on the page level.

## 5. Responsive rules in use

| Rule | Location |
|---|---|
| Quick-action columns: 1 (< 640), 2 (< 1040), 4 (≥ 1040) | `quick_actions.rs:152-160` |
| Rail stacked below 900 px window | `app.rs:25107` |
| Content padding: 32 (≥ 1440), 16 (≤ 1024), else 24 | `app.rs:25160-25166` |
| `is_compact` ≤ 1024 / `is_medium` 1024–1280 / `is_large` ≥ 1440 | `design_tokens.rs:230-245` |
| Sidebar width 288–320 clamp | `design_tokens.rs:223-228` (`sidebar_width_for`) |
| Two-thirds / one-third rail split (`FillPortion(2)` / `FillPortion(1)`) | `app.rs:25131-25134` |

Note: `is_medium` is `1024 < w < 1280`, so 1280 ≤ w < 1440 matches none of the three bands and
falls into the 24 px `else` branch (`app.rs:25164-25165`). Minor gap, worth normalizing in UI-HOME-02.

## 6. Existing shared design-system components (reusable)

| Module | Contents |
|---|---|
| `design_tokens.rs` | color roles (surface/border/text/primary…), spacing `SPACE_2..40` (:128-139), radii `RADIUS_SM/MD/LG/XL` (:146-149), shadows, breakpoints, `card_style` (:659), `elevated_style` (:673), `icon_button` (:688) |
| `fonts.rs` | `Typography` enum (:194), `PAGE_TITLE=28` (:157), `source_sans`/`inter`/`manrope`/`raleway_extra_bold`/`jetbrains_mono` (:91-133), `load_fonts` (:334) |
| `icon_system.rs` | `Icon` enum, `IconSize` (Xs 16 / Sm 18 / Md 20 / Lg 24 / Xl 28, :200-221), icon builder |
| `ui_components.rs` | `card`, `elevated_card`, `icon_tile` (:313), `primary_button`, `secondary_button`, `ghost_icon_button`, `text_input_field`, `status_dot`, `badge`/`badge_owned` (:712/:743), `divider`, `list_row`, `empty_state`, `Avatar`, `section_header`, `tooltip`, `card_header`, `date_separator`, `system_event_chip`, `connection_footer` (:53), `chat_status_footer` (:113), `connectivity_notice`, `InlineError`, `LoadingSkeleton`, `gutter_scrollable` (:1497) |
| `card_shell.rs` | `CardShell` builder (:51-207), `CARD_ROW_HEIGHT=48` (:32), `DEFAULT_LIST_MAX_HEIGHT=180` (:38) |
| `boru_dialog.rs` | `BoruDialog` modal shell (header/body/footer), `BORU_DIALOG_WIDTH_STANDARD=560` (:38) |
| `form_components.rs` | `form_label`, `helper_text`, `error_text`, `FormSection`, `TextInput`, `TextArea`, `Select`, `SearchableSelect`, `checkbox_field`, `toggle_field`, `SelectablePeerRow`, `peer_list`, `SelectablePeerList`, `remove_chip`, `selection_summary`, `DialogFooter`, `destructive_button` |
| `component_gallery.rs` | developer gallery of every primitive (`Screen::Gallery`, Ctrl+Shift+G in debug) |
| `presentation.rs` | `relative_time_from_system`, `truncate_with_ellipsis` (safe code-point truncation, :219) |
| `connection_details.rs` | `ConnectionDetailsViewModel` + dialog view (data-only formatting/redaction) |

## 7. Reusable vs duplicated vs flow-specific

- **Reusable (shared):** all of §6 — design tokens, typography, icons, ui_components primitives,
  `CardShell`, `BoruDialog`, `form_components`.
- **Duplicated:**
  - `DashboardTab` enum exists **twice**, identically: `dashboard.rs:11-37` and
    `dashboard_view_model.rs:11-38` (File-Sharing-only; candidates for consolidation).
  - Spacing: `SPACE_2`/`SPACE_6`/`SPACE_10` re-defined locally in `app.rs:534/541/542` while the
    rest are re-exported from `design_tokens` (`app.rs:536-539`).
  - Typography: `TYPO_*` are re-exports of `fonts::LG/MD/SM/XL/XS/XXS` (`app.rs:311-313`), so two
    naming systems coexist (`TYPO_*` vs `Typography::*`).
  - `count_badge` in `card_shell.rs:212-229` intentionally mirrors `ui_components::badge(…, BadgeKind::Accent)`
    (comment at :209-211 acknowledges the duplication).
- **Flow-specific (home screen only):** `view_chat_list_content` and its inline hero/mesh/header
  composition, `home_connection_variant` (pure, tested), the three rail selectors + card views,
  `quick_actions.rs` `ACTIONS`/card style, `connection_footer` usage.

## 8. Typography/fonts status vs plan

- Loaded at startup: Source Sans 3 (Regular/Semibold/Bold), Raleway ExtraBold (wordmark),
  JetBrains Mono (Regular/Italic) — `fonts.rs:334-343`.
- Bundled but NOT loaded: Inter, Manrope (`fonts.rs:331-333`).
- **Figtree is not present at all** — the plan's chat-message/composer font is a later task (typography epic), not UI-HOME-01 scope.
- Font files are compiled-in bytes; no font files are committed or distributed as artifacts in this audit.

## 9. Baseline screenshots (evidence)

Captured 2026-08-05 with the headless Xvfb pattern (`scripts/ui08_home_hero_screenshots.sh` /
`scripts/ui11_home_evidence.sh`): fresh temp data dir, `--no-dht --no-relay`, no subcommand →
lands on ChatList (home). Fresh-launch connecting state, truthful empty rail cards.

| Viewport | File |
|---|---|
| Wide 1600×900 | `docs/ui-redesign/evidence/t_0cda9247/t_0cda9247_home_1600x900_baseline.png` |
| Medium 1280×800 (reference) | `docs/ui-redesign/evidence/t_0cda9247/t_0cda9247_home_1280x800_baseline.png` |
| Narrow 1024×720 | `docs/ui-redesign/evidence/t_0cda9247/t_0cda9247_home_1024x720_baseline.png` |

Visual confirmation: at 1280×800 the quick-action grid is 4 columns; the "Create Public Room" /
"Create Group Chat" / "Add Friend" / "Share Files" labels wrap to two lines and the muted
descriptions below them are cut off at the card bottom — matching the §4 math (needs ≈122 px,
only 108 px available). At 1600×900 and 1024×720 the clipping is absent or marginal.

## 10. Recommended reuse points for downstream cards

1. **UI-HOME-02 (page container/grid/header):** reuse `design_tokens` spacing/radii + the existing
   `content_padding`/`rail_stacked`/`FillPortion` responsive machinery in `app.rs:25104-25186`;
   fix the `is_medium` band gap (`design_tokens.rs:235`); keep `gutter_scrollable` as the page
   scroller.
2. **UI-HOME-03 (dashboard card foundation):** `CardShell` (`card_shell.rs`) is the foundation —
   extend it (e.g. configurable header density, body padding) instead of new shells; route all
   three rail cards through it (already done) and migrate hero/mesh cards to `card_style`.
3. **UI-HOME-06 (quick-action clipping):** remove/relax `quick_actions.rs:88` fixed height;
   content-driven height keeps the icon-tile rhythm (`ui_components::icon_tile`); consider 2×2
   grid at ≥ 1040 or wider cards; keep `ACTIONS` static + real messages.
4. **UI-HOME-07/08 (Mesh Health / Online Peers / Recent Activity / Tunnels):** reuse the
   per-card selectors (`app.rs:24343-24437`) and `iced::widget::lazy` memoization pattern;
   rows already share `CARD_ROW_HEIGHT` (48) / 32 px activity rhythm.
5. **Typography tasks:** centralize on `fonts::Typography`; retire the `TYPO_*` aliases
   (`app.rs:311-313`) once the plan's type ramp lands.

## 11. Likely files downstream tasks will modify

- `examples/iced_chat/app.rs` — page container/header/grid (UI-HOME-02), rail card internals, quick-action placement, hero/mesh polish (UI-HOME-04/05/07/08)
- `examples/iced_chat/quick_actions.rs` — clipping fix + grid (UI-HOME-06)
- `examples/iced_chat/card_shell.rs` — shared card foundation (UI-HOME-03)
- `examples/iced_chat/design_tokens.rs` — spacing/radius/breakpoint tokens (UI-HOME-02/03)
- `examples/iced_chat/fonts.rs` — typography system incl. Figtree/Manrope wiring (UI-HOME-09/10)
- `examples/iced_chat/ui_components.rs` — shared primitives touched by all of the above
- `examples/iced_chat/boru_dialog.rs`, `form_components.rs`, `connection_details.rs` — dialog/form chrome reuse
- `examples/iced_chat/component_gallery.rs` — gallery entries for new/changed components
- `examples/iced_chat/dashboard_view_model.rs` / `recent_activity_view_model.rs` / `activity_log_view_model.rs` — only if the home rail adopts File-Sharing view models (currently separate)

## 12. What must remain untouched (business logic)

Networking/discovery/chat/room/group/file-sharing/tunnel logic in `src/` and the state machinery in
`app.rs` (subscriptions, selectors, message handlers). The audit did not modify any of it.

## 13. Remaining risks / notes for downstream cards

- The 132 px quick-action height is the only hard clip on the home screen; the rest are bounded
  scroll lists (intentional per card_shell.rs:8-12).
- 4-column quick-action grid at ≥ 1040 px is the layout pressure point; verify any fix at
  1280×800 first (reference viewport), then 1600 and 1024.
- `DashboardTab` duplication (`dashboard.rs` vs `dashboard_view_model.rs`) is pre-existing and
  File-Sharing-only; consolidate only if a task explicitly touches it.
- Dark mode uses the same layout; screenshots here are light-mode (fresh default).
- Evidence naming follows `<task-id>_<screen>_<width>x<height>_<state>.png` per
  `docs/ui-redesign/evidence/INDEX.md`.
