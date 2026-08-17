# Sender Screen-Sharing UI — Audit (BORU-SSUI Task 1)

Status: audit only — **no production code changed** in this task.

> **DEVELOPER NOTE (applies to the whole BORU-SSUI chain):**
> This is a presentation and interaction redesign, NOT a screen-sharing protocol
> rewrite. The sender redesign **must not alter capture/session behavior**: do not
> change anything under `src/screen_share/*` (capture, encode, codec negotiation,
> transport, permissions, remote input, audio capture), do not change the semantics
> of `HostCommand`, and do not duplicate session state inside individual controls.
> Bind the redesigned UI to the existing authoritative screen-sharing state model
> described below and dispatch the existing `AppMessage` variants. Only dispatch
> source/quality/audio events when user intent changes — never during redraws.

---

## 1. Files / functions to change (later tasks in the chain)

### Sender-side view builder
- `examples/iced_chat/app/chat.rs` — `view_screen_share_panel()` (line ~438).
  This one function hosts BOTH the sender and the viewer branches:
  - Sender branch: lines ~453–753 (the current raw controls).
  - Viewer branch: lines ~754+ (identity line, scalable surface, toolbar).
  - The panel is pushed into the conversation column directly below the
    conversation header + divider (`chat.rs` ~102–106), which matches Task 2's
    "below the conversation header" placement requirement.
- `examples/iced_chat/app/chat.rs` — `view_screen_share_fullscreen()` (line ~957):
  viewer fullscreen overlay; shares the same control row helper.

### Viewer-side primitives (reuse, do not rebuild)
- `examples/iced_chat/app/screen_share_surface.rs`:
  - `view_screen_share_surface()` (line ~201) + `SurfaceGeometry` — scalable
    surface with fit/zoom/pan and normalized remote-input mapping.
  - `view_screen_share_view_controls()` (line ~357) — the compact viewer toolbar
    row (Fit / 100% / − / + / Reset / Cursor / Fullscreen); `.wrap()`s at narrow
    widths. The sender redesign should mirror this control language.
  - `screen_share_metrics_lines()` (line ~428) / `view_screen_share_metrics_overlay()`
    (line ~476) — dev diagnostics; retain even if not shown in the new card.

### Message variants + update handlers (dispatch targets)
- `examples/iced_chat/app.rs` — `AppMessage` variants, `#[cfg(feature = "screen-sharing")]`,
  lines ~5788–5868:
  - `StartScreenShare(PublicKey)`, `StopScreenShare`, `AcceptScreenShare`,
    `DeclineScreenShare`, `ScreenShareSelectSource(CaptureSourceId)`,
    `ScreenShareSetPreset(Option<QualityPreset>)`, `ScreenShareToggleAudio`,
    `ScreenShareGrantControl(Vec<Capability>)`, `ScreenShareDenyControl`,
    `ScreenShareRevokeControl`, `ScreenShareHostSendClipboard`,
    `ScreenShareLowerQuality` / `ScreenShareFullQuality` (viewer side),
    `ScreenShareDismissNotice`, `ScreenShareEventReceived(SessionEvent)`.
- `examples/iced_chat/app.rs` — update handlers, lines ~14906–15093:
  - `StopScreenShare` (~14908), `ScreenShareGrantControl` (~15029),
    `ScreenShareToggleAudio` (~15042), `ScreenShareRevokeControl` (~15053),
    `ScreenShareSelectSource` (~15064), `ScreenShareSetPreset` (~15078),
    `ScreenShareDismissNotice` (~15089).
- `examples/iced_chat/app.rs` — private helpers:
  - `start_screen_share(peer)` (~22320) — spawns the host thread, sets
    `ScreenShareHostState::Requesting`, creates the `HostCommand` channel.
  - `send_screen_share_quality(...)` (~22444) — viewer-side manual quality.
  - `apply_screen_share_event(event)` (~22760) — THE state->UI bridge: maps
    `SessionEvent` → presentation fields.
  - `reset_screen_share_state()` (~23039) — teardown of all UI flags.

### Authoritative state (do NOT modify in this chain)
- `src/screen_share/host.rs` — `run_host_session`, `HostCommand` (line ~51):
  `GrantControl`, `RevokeControl`, `SendClipboard`, `SwitchSource`,
  `SetAudioEnabled`, `SetQualityPreset`.
- `src/screen_share/session.rs` — `SessionManager`, `SessionState`, `SessionEvent`
  (line ~52).
- `src/screen_share/permissions.rs` — `Capability` (line ~27), permissions.
- `src/screen_share/capture.rs` — `CaptureSource`, `CaptureSourceId`,
  `CaptureSourceKind`, `picker_label()` (line ~236).
- `src/screen_share/presets.rs` — `QualityPreset` (line ~27) + `QualityProfile`.
- `src/screen_share/transport.rs` — `PathKind` (line ~39).
- `src/screen_share/stats.rs` — `ScreenShareSessionMetrics` (line ~94),
  `ScreenShareStatsSnapshot`.

---

## 2. Control → message/event/update trace + authoritative vs presentation state

| Current control (sender panel) | Dispatches | Update handler | Authoritative state | Presentation state (UI mirror) |
|---|---|---|---|---|
| Source picker (raw text buttons, `✓` marker) | `ScreenShareSelectSource(source.id)` | app.rs ~15064 → `screen_share_selected_source = Some(id)` + `HostCommand::SwitchSource(id)` | Host driver's active capture source; available list comes from `SessionEvent::SourcesEnumerated` (app.rs ~22951) | `screen_share_sources` (list), `screen_share_selected_source` (marker; updated optimistically + on `SourceChanged`) |
| Quality preset buttons (LAN High / Balanced / Relay / Auto) | `ScreenShareSetPreset(Some(QualityPreset))` / `(None)` | app.rs ~15078 → `HostCommand::SetQualityPreset` | `ScreenShareSessionMetrics.preset` + `path_kind` + `adaptive_level` (published ~1 Hz via `SessionEvent::Metrics`, app.rs ~23026) | `screen_share_host_metrics` (label line) |
| Remote-control status text (ON/OFF) | (read-only; consent buttons: `ScreenShareGrantControl(Vec<Capability>)`, `ScreenShareDenyControl`, `ScreenShareRevokeControl`) | app.rs ~15029/15037/15053 → `HostCommand::GrantControl/RevokeControl` | `SessionEvent::ControlChanged { active, capabilities }` (app.rs ~22912) | `screen_share_control_active`, `screen_share_control_request` (pending prompt), `screen_share_clipboard_active` |
| Audio toggle (raw button "Audio On/Off") | `ScreenShareToggleAudio` | app.rs ~15042 → `HostCommand::SetAudioEnabled(!screen_share_audio_active)` | `SessionEvent::AudioState { enabled, error }` (app.rs ~22929) — host-side; `ControlChanged` caps for viewer | `screen_share_audio_active` |
| Stop Sharing (raw button) | `StopScreenShare` | app.rs ~14908 → stop flags + `ControlMessage::EndSession` + `reset_screen_share_state()` + `state = Stopped` | Host task stop flag (`screen_share_host_stop`); session manager state | `screen_share_host_state` |
| Share Again / Dismiss (terminal states) | `StartScreenShare(key)` / `ScreenShareDismissNotice` | app.rs ~14906 / ~15089 | — | `screen_share_host_state == Stopped/Error` |
| State line ("Requesting…", "Sharing with …") | — (derived) | `SessionEvent::Accepted/Rejected/Reconnecting/Reconnected/Ended/SourceUnavailable` | `SessionEvent` lifecycle | `screen_share_host_state` (enum, app.rs ~3678: Idle/Requesting/Inviting/Streaming/Paused/Reconnecting/Stopped/Error) |

### What is authoritative (bind UI to this; do not duplicate)
- Session lifecycle and permissions: `SessionManager` / `SessionState` /
  `SessionPermissions` inside `src/screen_share/session.rs` — the protocol layer's
  source of truth.
- Which source is really captured: the host driver (`run_host_session`), driven
  by `HostCommand::SwitchSource`.
- Effective quality: `QualityPreset` in `ScreenShareSessionMetrics` (host
  streaming loop), which reflects both the path-derived auto preset and the
  user's override.
- Control / clipboard / audio grants: `Capability` list in `SessionEvent::ControlChanged`
  (viewer) and `SessionEvent::AudioState` (host). Audio is a separate opt-in
  capability — never implied by remote control.
- Remote input handling (pointer/keyboard/wheel): `remote_input.rs` + viewer-side
  `ScreenSharePointerMove/Button/Wheel/KeyEvent` messages.

### What is presentation-only (safe to restyle / reorganize)
- `screen_share_selected_source` (optimistic marker; real switch is the host).
- `screen_share_host_state` (UI lifecycle mirror, driven by `SessionEvent`).
- `screen_share_invite`, `screen_share_control_request` (pending prompt),
  `screen_share_notice_ticks` (notice auto-clear timer).
- `screen_share_dev_overlay` (debug gate: `--dev-ui` / `BORU_DEV_UI=1`).
- Viewer-only presentation: `screen_share_view_mode`, `screen_share_pan`,
  `screen_share_drag`, `screen_share_hover`, `screen_share_fullscreen`,
  `screen_share_cursor_enabled`, frame handle / `src_size` / cursor sprite cache.

### Emit-once rule
The handlers above (`ScreenShareSelectSource`, `ScreenShareSetPreset`,
`ScreenShareToggleAudio`) are the only dispatch points for source/quality/audio.
The redesigned view must call them only on user intent — never from redraw/
`view_*` code.

---

## 3. Reusable viewer-side shared primitives

BORU-SS built these for the viewer toolbar / surface; the sender redesign should
reuse them (Task 12 of the PDF asks to extract/share them where both sides share
a concept):

1. **Scalable surface** — `view_screen_share_surface()` + `SurfaceGeometry`
   (`screen_share_surface.rs` ~25–347): fit/zoom/pan, normalized coordinate
   mapping for remote control. Reuse as-is for any surface work.
2. **Compact control row** — `view_screen_share_view_controls()`
   (`screen_share_surface.rs` ~357): the viewer toolbar (Fit/100%/−/+/Reset/
   Cursor/Fullscreen), uses `.wrap()` for narrow panes. This is the visual
   language the sender card should match ("one coherent feature family").
3. **Metrics/dev overlay helpers** — `screen_share_metrics_lines()` +
   `view_screen_share_metrics_overlay()` (shared by host panel and viewer).
4. **Generic themed widgets** (`ui_components.rs`):
   - `Card` (~205) / `elevated_card` (~327) — rounded container w/ padding +
     optional press; candidate for the Task 2 parent card and Task 3 source cards.
   - `primary_button` / `primary_button_icon` (~459/482), `secondary_button`
     (~526), `ghost_icon_button` (~553, has `destructive` + `dimmed` flags) —
     candidate for the Task 7 action row (Pause Preview neutral, Stop Sharing
     destructive).
   - `badge` / `badge_owned` (~750/781), `status_dot` (~708), `divider` (~815),
     `ListRow` (~832) — status chips and separators.
5. **Single-choice segmented pattern** — no dedicated segmented-control primitive
   exists yet; the closest pattern is the activity-log filter chips in
   `examples/iced_chat/app/files.rs` (~3282): a row of buttons where the active
   one gets `primary` fill + white text and inactive ones get `surface` fill +
   `text_secondary`, with hover via `surface_hover`. Task 4 should extract this
   into a shared `segmented_control` primitive (per Task 12).
6. **Icon system** — `examples/iced_chat/icon_system.rs` (`Icon`, `IconSize`,
   `ghost_icon_button` helper) for monitor/window/desktop/audio/stop icons.

---

## 4. TOML style tokens (current) + categories the chain will add

### Current token plumbing
- `examples/iced_chat/theme_config.rs`:
  - `config_group!` macro defines per-area config structs; `ChatConfig`
    (lines ~539–555) already contains `screen_share_w` / `screen_share_h`
    (lines 548–549) — currently only the viewer box geometry.
  - `UiThemeConfig` (line ~697) aggregates `Option<ChatConfig>` etc.; parsed by
    `parse_ui_theme_config` / loaded via `load_ui_theme_config` /
    `reload_ui_theme_config` (watcher hot-reload).
- `examples/iced_chat/theme.rs` — `ChatTheme` (line ~1339) mirrors the config
  with defaults (`screen_share_w: 640.0`, `screen_share_h: 360.0`, lines
  1396–1397).
- `examples/iced_chat/theme_merge.rs` — `merge_chat_theme` (line ~560) maps
  `ChatConfig` → `ChatTheme` with clamping (`clamp_size0`).
- `boru-ui.example.toml` — the documented override file; `[chat]` section shows
  `# screen_share_w = 640.0` / `# screen_share_h = 360.0` (lines 303–304).
- Layout (separate system, BORU-LAYOUT): `examples/iced_chat/layout.rs`
  `ScreenShareLayout` (line ~652) holds viewer box `width`/`height` read via
  `self.boru_layout().chat.screen_share` (used at chat.rs ~787).

### Shared design tokens already available (design_tokens.rs)
Spacing `SPACE_2…SPACE_40` (141–153), radii `RADIUS_SM/MD/LG/XL/CARD` (160–167),
`BORDER_WIDTH`, `FOCUS_WIDTH`, `CONTROL_HEIGHT` / `CONTROL_HEIGHT_COMPACT`,
color fns: `surface`, `surface_hover`, `surface_pressed`, `surface_selected`,
`surface_secondary`, `border_muted`, `border_strong`, `border`, `text_primary`,
`text_secondary`, `primary`, `primary_hover`, `primary_pressed`, `primary_soft`,
`destructive`, `destructive_soft`, `focus_border`, `surface_style`.

### Token categories the chain will add (Task 8)
Add a new `ScreenShareConfig` config group + `[screen_share]` TOML section in the
same pattern as `ChatConfig`/`[chat]`, merged into a new `ScreenShareTheme`
sub-struct on `BoruTheme` (alongside `ChatTheme`). Suggested categories (from the
PDF):
- `screen_share.card.*` — parent card surface, border, radius, shadow, padding.
- `screen_share.source_card.*` — source card surface, selected accent/border,
  hover/pressed, min width, icon size.
- `screen_share.segmented.*` — segmented control radius, selected/unselected
  fills, spacing.
- `screen_share.toggle.*` — audio switch geometry and track/thumb colors.
- `screen_share.action.*` — neutral action button (Pause Preview) tokens.
- `screen_share.destructive.*` — Stop Sharing destructive treatment (reuse
  existing `destructive` / `destructive_soft` colors; reserved fill for
  hover/pressed per Boru destructive conventions).

Rule: reuse `design_tokens.rs` / existing `[chat]`/spacing/radius values wherever
possible so the card matches the rest of Boru; do not scatter per-widget magic
numbers. Light/dark must stay token-driven (never bake white backgrounds / fixed
dark text).

**Status (BORU-SSUI-08): DONE.** `ScreenShareTheme` (theme.rs) with
`card` / `source_card` / `segmented` / `toggle` / `action` / `destructive`
sub-groups, `ScreenShareConfig` (theme_config.rs), and
`merge_screen_share_theme` (theme_merge.rs) now exist and are wired into
`BoruTheme` / `UiThemeConfig` / `merge_ui_theme`, so `boru-ui.toml` hot-reloads
the sender card geometry through the same system as the rest of the redesigned
UI (`boru-ui.example.toml` documents every key). The sender widgets in
`chat.rs` consume the tokens (`screen_share.card.*` shell + rhythm,
`source_card.*` cards, `segmented.*` quality control, `toggle.*` audio row,
`action.*` / `destructive.*` action row); the shared primitives
`ui_components::segmented_control` and
`form_components::destructive_button_icon` take caller-supplied style structs
(`SegmentedControlStyle` / `DestructiveButtonStyle`) so TOML overrides reach
them. Colours remain mode-aware `design_tokens` calls (no baked-in white
backgrounds / fixed dark text). `IconSize::from_px` maps the TOML px icon-size
tokens to the nearest bundled icon class.

---

## 5. Appendix — exact current sender control rendering (baseline)

The sender branch of `view_screen_share_panel()` currently renders a plain
`column(items)` with `SPACE_6` spacing — **no card container, no themed border**.
Items in order:
1. State line (`screenshare.requesting/awaiting_acceptance/sharing_with/…`).
2. Error reason text (if `Error(reason)`).
3. Source row: label `Source:` + raw `button` per source, `✓` prefix on the
   selected one, `padding([2,6])` (chat.rs ~504–534).
4. Remote-control status text (`Remote control: ON/OFF`) while Streaming.
5. Quality line (`Quality: {preset} · Path: {path} · Adaptive L{level}`) from
   `screen_share_host_metrics` (~555–585).
6. Preset buttons row (`LAN High / Balanced / Relay / Auto`), `padding([2,6])`
   (~589–623).
7. Control-request consent prompt + grant/deny buttons (~626–673).
8. Remote-control-active line + Revoke button (~674–684); clipboard button
   (~685–691).
9. Audio toggle raw button `Audio On/Off` while Streaming (~698–708).
10. Dev overlay metrics lines (if `screen_share_dev_overlay`) (~711–722).
11. Terminal state: Share Again + Dismiss; else Stop Sharing button (~726–752).

Localized strings live in `examples/iced_chat/locales/en.json` under
`screenshare.*` (keys ~594–642, e.g. `preset_lan_high` "LAN High",
`preset_auto` "Auto", `remote_control_on` "Remote control: ON",
`stop_sharing` "Stop Sharing", `sharing_with` "Sharing your screen with {name}").
The redesigned card must keep using these keys (actual runtime labels) rather
than copying mockup text.

---

## 6. Out of scope / guardrails (restated)

- No changes to `src/screen_share/*` (capture/session/network/codec/permissions).
- No new `AppMessage` variants or `HostCommand`s unless a real defect is exposed;
  prefer reusing existing dispatch paths.
- Sender-only semantics stay separate from viewer-only semantics (Task 12); share
  presentation primitives, not state machines.
- Stop-sharing cleanup (stop capture, release resources, EndSession, reset UI)
  must remain exactly as `StopScreenShare` / `reset_screen_share_state` implement
  it today.

---

## 7. Task 9 (Responsive layout) — status: DONE

BORU-SSUI-09 implements the PDF Task 9 responsive behavior for the sender
screen-share control card, reusing the existing responsive tier machinery
(`LayoutConfig::responsive` / boru-layout.toml `[responsive]`) instead of
inventing new breakpoints.

### What changed

- **One responsive control row.** The quality segmented control (SSUI-04),
  remote-control status (SSUI-05) and audio toggle (SSUI-06) are now built by
  three extracted helpers (`view_screen_share_quality_group`,
  `view_screen_share_remote_status_group`, `view_screen_share_audio_group`) and
  combined by `view_screen_share_control_row` into ONE responsive row:
  - **UltraWide** (≥ `ultra_wide_min_width`, default 1440): all three groups
    share one horizontal row (`SenderControlRowLayout::Row`).
  - **Desktop** (360–1439): the same row may wrap into two logical groups
    without clipping (`SenderControlRowLayout::Wrap`, `row.wrap()`).
  - **Narrow** (< `narrow_max_width`, default 360): the groups stack vertically
    (`SenderControlRowLayout::Stack`).
  - The tier is resolved from the *panel's actual measured width* via
    `self.boru_layout().responsive.tier_for_width(size.width)` inside a
    `widget::responsive` closure — no app-wide responsive machinery was
    modified (BORU-RESP owns that).
  - **`Responsive` height pitfall (found during verification):** iced 0.14's
    `responsive` widget defaults to `height: Length::Fill`. Inside the card's
    Shrink-height items column the flex layout then allocates it the REMAINING
    height, which at 640 px was only ~49 px — the Stack column was squashed so
    the segmented control collapsed to 9.9 px and the remote/audio groups to
    0 px (invisible). Forcing `.height(Length::Shrink)` on the responsive row
    lets it size to its content's natural height at every tier; the closure
    still receives the full measured width for tier resolution.
  - `SenderControlRowLayout::for_tier` is a pure mapping, unit-tested.
- **Source row stays scrollable** (SSUI-03 already made it a horizontal
  scrollable), so medium/narrow widths keep every source reachable; at wide
  widths all cards fit in one row. No change needed.
- **Long peer names ellipsize.** The card title ("Sharing your screen with
  {name}") truncates the peer name with `truncate_with_ellipsis` using the new
  `screen_share.card.title_max_chars` token (default 32) and renders it in a
  clipped no-wrap container, so a long name can never wrap, overlap the
  controls, or spill outside the card. Window titles already ellipsized via the
  source-card `title_max_chars` token (SSUI-03); a regression test pins both.
- **Sensible minimum widths.** Source cards keep their fixed 192 px width
  (`screen_share.source_card.width`) — a sensible minimum that prevents tiny
  chips. Buttons (segmented segments, action buttons, destructive Stop) keep
  their tokenized padding, which guarantees a usable hit area; iced 0.14 has no
  native min-width primitive, so exact `Length::Fixed` forcing was deliberately
  avoided to prevent clipping longer localized labels. The app-wide
  `viewport_min_width` (1024) was NOT raised — the card adapts instead of
  forcing a large window minimum.
- **Actions stay left/right aligned** — the action row keeps the fill spacer +
  right-aligned destructive Stop Sharing (SSUI-07), unchanged at all widths.

### TOML tokens added

- `screen_share.card.title_max_chars` (32.0) — documented in
  `boru-ui.example.toml` `[screen_share.card]`.

### Verification

- `rb check` passes for both `gui,video-playback,terminal` and
  `+screen-sharing` (pre-existing warnings only).
- Targeted `rb test` on debsrv: new
  `sender_control_row_layout_maps_tiers_to_modes`,
  `sharing_with_title_ellipsizes_long_peer_name`,
  `source_card_title_budget_ellipsizes_long_window_titles`; plus the SSUI-08
  `screen_share_geometry_matches_design_tokens` /
  `screen_share_geometry_is_mode_independent` /
  `screen_share_tokens_merge_and_clamp` and the full 22-test screen_share
  suite — all pass.
- Offscreen capture tests render the same streaming session at the three PDF
  window sizes plus a long-peer-name variant
  (`capture_screen_share_sender_card_maximized_1920` /
  `_reference_1280` / `_narrow_split` / `_long_peer_name`) and write
  `captures/screen_share_sender_card_*.png`; a layout-tree walk at 640 during
  verification confirmed the Stack column gives all three groups real heights
  (no 0-px collapse after the `.height(Shrink)` fix).
- Manual layout checks at ~1280x800, a narrow split-window, and a maximized
  1920x1080+ window documented in the task result.