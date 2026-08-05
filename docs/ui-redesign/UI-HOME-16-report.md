# UI-HOME-16 — Intentional empty states

- Task: `t_4186e7f9` (UI-HOME-16)
- Repo: `iroh-gossip-chat` @ worktree `wt/t_4186e7f9`
- Status: IMPLEMENTED + VERIFIED (build, 896 tests, per-card screenshot evidence, OCR + pixel checks)

## Summary

Every list-oriented home card now has an intentional empty state rendered
with a small muted icon + muted supporting text, using the shared card and
typography systems. The Online Peers / Recent Activity / Tunnels rail cards
and the Mesh Health "Recent events" section all show the exact approved copy
(or the retained connection summary for Mesh Health), wrap correctly at
narrow widths, keep relevant actions available, and stay balanced — never a
blank panel, never an oversized card. Live-state transitions in and out of
the empty states are preserved (the selectors/dependency slices are
unchanged; only the render changed, verified by the populated screenshot and
the existing dependency-isolation tests).

## What changed

### `examples/iced_chat/card_shell.rs` — shared empty-state treatment

- New `CardShell::empty_icon(element)` builder (UI-HOME-16): an optional
  caller-owned icon rendered left of the `empty_message` text.
- The built-in empty state now renders a horizontal row — small icon (when
  supplied) + `SupportingText` muted, with `Length::Fill` + word wrapping so
  two-sentence copy reflows at narrow rail widths instead of overflowing.
  Vertical padding stays restrained (`SPACE_8`), so empty cards never grow
  excessively tall.
- The shell stays data-agnostic: callers own the icon element (e.g. an
  `icon_svg` glyph tinted `text_muted`).
- New tests: `card_shell_stores_empty_icon`,
  `card_shell_build_empty_state_with_icon_does_not_panic`,
  `card_shell_build_empty_state_without_icon_does_not_panic`.

### `examples/iced_chat/app.rs` — card empty states

- New copy constants (exact spec):
  - `ONLINE_PEERS_EMPTY_MESSAGE` = "No peers are online right now. Connected peers will appear here."
  - `RECENT_ACTIVITY_EMPTY_MESSAGE` = "No recent activity. Network events will appear here." (was "No recent activity")
  - `TUNNELS_EMPTY_MESSAGE` unchanged ("No active tunnels. Create or join a tunnel to securely route traffic.")
  - `MESH_EVENTS_EMPTY_MESSAGE` = "No recent mesh events"
- **Online Peers** (`view_online_peers_card`): the empty body is now a
  muted friend icon + the spec copy, vertically centred in the existing
  `PEERS_BODY_MIN` (128 px) floor — the card keeps its ~220–280 px footprint
  (never a tiny strip, never a huge blank panel), text wraps.
- **Recent Activity** (`view_recent_activity_card`): passes
  `.empty_icon(ICON_ACTIVITY muted)` + the new copy to CardShell.
- **Tunnels** (`view_tunnels_card`): passes `.empty_icon(ICON_LOCK muted)`
  + the existing copy; the header action label is now truthful per state —
  **"Create tunnel"** when the list is empty (the dialog the copy points
  at), **"View all"** once live tunnels exist (destination unchanged:
  `ShowCreateTunnelDialog`). New pure helper `tunnels_header_action_label`.
- **Mesh Health** (`view_chat_list_content`, Recent events): the
  no-events branch keeps the connection summary + stat tiles + lobby line
  above and renders `MESH_EVENTS_EMPTY_MESSAGE` with the same small
  icon + muted + wrapping treatment.
- Tests: `home_rail_empty_state_copy_matches_ui_home_16_spec` (pins all
  four constants; renamed from the UI-HOME-08 copy test),
  `home_rail_empty_cards_build_with_ui_home_16_states` (builds the three
  rail cards empty + the full home screen without panic),
  `tunnels_header_action_label_switches_to_create_tunnel_when_empty`
  (empty → "Create tunnel", live tunnel → "View all").

### `examples/iced_chat/mcp_server.rs` — test harness action (evidence only)

- New `boru_gui_clear_mesh_events` GUI test action (gated on
  `--enable-gui-test-actions`, no params, rate-limited like its siblings):
  queues the existing `GuiTestCommand::ClearMeshEventLog` so evidence
  harnesses can capture the Mesh Health no-events state. It only empties
  the live bounded mesh event log — it never fabricates events. The
  same state is reachable in production when the UI-28 watchdog purges
  transient startup lines after the mesh goes Good.

### `DESIGN_SYSTEM.md`

- §4.2 Dashboard Card: documents the empty-state convention (small muted
  icon via `empty_icon` + muted `SupportingText`, word wrapping).

### Evidence harness

- `scripts/ui_home16_empty_states_evidence.sh` (Xvfb + MCP + OCR, same
  pattern as UI-HOME-08/15): captures the fresh-launch empty state at
  1280×800, per-card crops, the Mesh Health no-events state (via the new
  harness action), the 800×600 minimum band (top + scrolled), and a
  populated capture (seeded fixture + real `boru_gui_set_peer_presence`
  events through the production `handle_friend_event → push_activity`
  path) proving the live transition out of empty.
- Evidence under `docs/ui-redesign/evidence/t_4186e7f9/`: 10 PNGs +
  `ocr.txt` + `README.md`.

## Verification

- `cargo build --example boru --features gui` — OK (exit 0; only the 207
  pre-existing warnings, untouched).
- `cargo test --example boru --features gui` — **896 passed / 0 failed**
  (prior 891; +5 net new: 3 card-shell + 2 app.rs; the UI-HOME-08 copy test
  was renamed to the UI-HOME-16 spec test).
- Screenshot evidence (OCR-verified):
  - Online Peers crop: exact copy, muted friend icon, balanced min-height body.
  - Recent Activity crop: exact copy, muted activity icon, wraps across two lines in the rail.
  - Tunnels crop: exact copy, muted lock icon, "Create tunnel" header action.
  - Mesh Health (cleared log): connection summary + stat tiles + lobby retained; "No recent mesh events" with muted icon below the divider.
  - 800×600 scrolled: compact two-line headers and the rail copy wraps in the narrow band (OCR "peers are online" / "recent activity" / "active tunnels" all present).
  - Populated 1280×800: ONLINEPEERS badge 1/3 and RECENT ACTIVITY 5 with real rows ("Alice came online", "Bob went offline", …) — live transition out of empty works.
- Geometry: `words_past_right_edge=0` at all four OCR'd captures (no horizontal overflow).
- Pixel checks: each rail card's body shows a small glyph cluster in the
  left icon strip (connected-component analysis at 72% threshold) — the
  muted icons render, not just the text.
- Accessibility semantics: iced 0.14 buttons are not keyboard-focusable and
  `button::Status` has no `Focused` variant (pre-existing framework
  limitation, unchanged); the empty-state copy is real `Text` (screen-reader
  readable), muted colours keep 4.5:1+ contrast against the card surface per
  the existing design-token palette.

## Changed files

- `examples/iced_chat/card_shell.rs` — `empty_icon` builder + empty-state render + 3 tests
- `examples/iced_chat/app.rs` — copy constants, Online Peers/Recent Activity/Tunnels empty renders, Tunnels action label helper, Mesh Health events empty render, 3 test changes
- `examples/iced_chat/mcp_server.rs` — `boru_gui_clear_mesh_events` harness tool (dispatch + params + handler + test)
- `DESIGN_SYSTEM.md` — empty-state convention note
- `scripts/ui_home16_empty_states_evidence.sh` — new evidence harness
- `docs/ui-redesign/UI-HOME-16-report.md` — this report
- `docs/ui-redesign/evidence/t_4186e7f9/` — 10 PNGs + `ocr.txt` + `README.md`

No networking/discovery/chat/room/group/file-sharing/tunnel business logic
touched.

## Acceptance criteria

- Every list-oriented card has an intentional empty state — yes (Online Peers, Recent Activity, Tunnels, Mesh Health Recent events).
- Empty cards remain balanced — yes (Online Peers keeps the 128 px min-height floor; the others are content-sized with restrained padding).
- Text wraps correctly — yes (word wrapping on all empty-state text; verified at 1280 rail and 800 minimum band).
- Relevant actions remain available — yes (Online Peers "View all" → friend requests; Tunnels "Create tunnel" → Create Tunnel dialog; Mesh Health "View details" → connection details).
- Live transition behaviour works — yes (renders read the unchanged selectors; populated capture + existing dependency-isolation tests).

## Remaining risks / notes

- The mesh no-events screenshot is produced with the new
  `boru_gui_clear_mesh_events` test action because a fresh `--no-dht
  --no-relay` launch always pushes real startup lines into the mesh log
  (which is truthful — the log is simply not empty in that harness
  configuration). In production the identical state appears after the
  UI-28 watchdog clears transient lines once the mesh is Good.
- Full-page OCR of small muted 13 px text is noisy (tesseract merges
  words); the copy checks therefore run against the card crops, which OCR
  cleanly. The crops are the evidence; full-page captures prove layout
  and geometry.
- The 800×600 top capture intentionally shows the rail below the fold
  (one-column layout stacks the rail under the main column); the scrolled
  capture proves the rail and its wrapping.
- Pre-existing: ~207 build warnings, `cargo fmt` drift in app.rs /
  card_shell.rs (untouched, matches prior-card precedent),
  `design_tokens.rs` NUL-byte history (file valid UTF-8).
