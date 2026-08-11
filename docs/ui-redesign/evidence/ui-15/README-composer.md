# t_929dfe1a — UI-15 Modern message composer

Replaces the plain bottom input strip with an elevated, rounded composer
(Phase 4, Figure 4) while preserving every send/attachment/draft behavior.
All changes are presentation + state-machine only: message records, ordering,
persistence, permission and authorization semantics are untouched.

## What changed

- **Composer surface** — 16 px radius (`RADIUS_XL`) container with a subtle
  1 px `border_muted` border, `bg_surface_secondary` fill and a soft card
  shadow (`shadow_card`, plan §4 "elevation ~0 1 2"). The whole bar sits
  above the connection footer and **outside** the timeline scroll region
  (fixed row in `view_chat_panel`: header → divider → responsive timeline →
  composer).
- **Figure 4 alignment** — `attach | input (fill) | GIF | emoji | send`.
  The attachment (paperclip) action moved to the leading edge; GIF and emoji
  pickers and the send action stay on the trailing edge.
- **Send button** — circular green (`BUTTON_PRIMARY_GREEN`, radius =
  `SPACE_18`) with a white lucide send glyph when text is present; muted
  transparent circle (disabled) when the composer is empty; green with a
  `…` spinner glyph while a broadcast is in flight (`composer_sending`).
  The sending flag is set when the send task starts and cleared by the
  chained `ComposerSendFinished` completion message.
- **IME composition** — `InputMethod` window events drive `composer_ime_active`.
  Only an **active preedit** sets the flag (fix c356d369): `Opened` merely
  means a text field gained focus, so it clears the flag instead of freezing
  the composer in environments without a real IME session. While composing,
  `SendPressed` is a no-op so the Enter that confirms the composition does
  not also submit the message.
- **File drag-over** — window file drag over/leave toggles `composer_drag_over`,
  which recolors the composer border to the accent color (subtle focus
  treatment). A drop routes through the same extension-based pipeline as the
  attachment button (image → `ExecuteImageSend`, other → `ExecuteFileSend`)
  and clears the drag-over flag.
- **Draft preservation** — unchanged per-conversation behavior: `composer_text`
  is stashed on the `ConversationLive` when leaving a chat and restored on
  re-open (app.rs `RoomOpened`/conversation-switch handlers).

## Preserved keyboard shortcuts & composer event handlers (worker handoff)

| Trigger | Handler | Behavior (unchanged unless noted) |
|---|---|---|
| `Enter` in composer | `text_input.on_submit → SendPressed` | Send current draft (trimmed; empty = no-op) |
| `Enter` during IME preedit | `SendPressed` + `composer_ime_active` guard | **No-op** — commits the composition instead of sending (new in UI-15, required by card) |
| Typing | `InputChanged` | Updates `composer_text`; completes `SetComposerText` test action |
| Paperclip button | `AttachPressed` | `rfd` file dialog → extension auto-detect → `ExecuteImageSend` / `ExecuteFileSend` |
| `GIF` button | `ToggleGifPicker` | Opens/closes GIF picker (unchanged) |
| Emoji button | `ToggleEmojiPicker` | Opens/closes emoji picker (unchanged) |
| Window file drag over | `ComposerDragOver(true)` | Sets accent border focus treatment |
| Window file drag leave | `ComposerDragOver(false)` | Restores `border_muted` |
| Window file drop | `ComposerFileDropped(PathBuf)` | Clears drag-over; routes by extension to image/file send |
| `InputMethod::Opened` | `ComposerImeActive(false)` | IME enabled on focus — does **not** block sending |
| `InputMethod::Preedit` | `ComposerImeActive(true)` | Active composition — blocks sending |
| `InputMethod::Closed` / `Commit` | `ComposerImeActive(false)` | Composition finished — sending re-enabled |
| Send task completion | `ComposerSendFinished` | Clears `composer_sending` (transient sending state off) |
| `/send`, `/image`, `/download` | `SendPressed` command parsing | Still routed through the normal send path (unchanged) |

## Verification

- Build: `cargo build --features gui --bin boru` — PASS.
- Tests: `cargo test --features gui --bin boru` — **655 passed, 0 failed**.
  UI-15-specific (all pass):
  - `send_pressed_skips_while_ime_composing`
  - `composer_sending_flag_roundtrips`
  - `composer_drag_over_and_file_drop_routing`
- MCP composer actions (`boru_gui_set_composer` / `boru_gui_submit_composer`)
  exercise the real update path; their `mcp_server` tests pass.

## Visual evidence (fresh captures, Xvfb)

1280×800 (reference):

- `ui15_empty_1280x800.png` — empty composer: paperclip left, placeholder
  text, muted disabled send circle.
- `ui15_typed_1280x800.png` — typed draft: green circular send with white
  send glyph.
- `ui15_attach_hover_1280x800.png` — paperclip hover state.
- `ui15_sending_1280x800.png` — transient sending state (broadcast held by a
  slow link-preview fetch so the spinner glyph is visible; composer cleared,
  "Sending / Loading preview…" bubble present).

1024×720 (compact width — input remains usable, no clipping/overlap):

- `ui15_empty_1024x720.png`, `ui15_typed_1024x720.png`,
  `ui15_attach_hover_1024x720.png`, `ui15_sending_1024x720.png` — same four
  states at the compact viewport.

Re-run: `bash scripts/ui15_composer_evidence.sh` (requires Xvfb, xdotool,
ImageMagick; builds/uses `target/debug/boru`). Set `BORU_WIDTH` /
`BORU_HEIGHT` to capture other viewports.

## Remaining risks / notes

- The GIF picker and emoji picker panels themselves are not exercised by the
  capture script (no MCP command toggles them in a seeded room); they are
  unchanged toggle buttons and their handlers are covered by existing tests.
- Long drafts: the input is a single-line iced `text_input` that fills the
  row (same widget as before); a very long draft scrolls inside the field and
  cannot overflow the composer row.
- The composer commits (542afa9d, 7932d776, c356d369) are on `main`; the
  implementation hunks were also recovered into the tree via the FS-08
  stash-recovery commit (58f2054b) — see commit message for attribution.
