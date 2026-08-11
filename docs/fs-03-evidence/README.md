# FS-03 — File Sharing route + persistent navigation entry (evidence)

## CARD / STATUS

- Card: FS-03 — Add the File Sharing route and persistent navigation entry
- Status: Implemented; build green; route-level tests pass; runtime/MCP verified
- Commit: see `git log -1` for the FS-03 commit
- Repo: /home/dan/iroh-gossip-chat (boru-core)

## WHAT WAS BUILT

1. `Screen::FileSharing` — new route in the existing `Screen` enum.
2. `AppMessage::OpenFileSharing` — navigation message handled in the normal
   update path. Navigation only: the shared shell, gossip subscriptions, and
   conversation forwarders stay alive; only the main panel swaps.
3. `view_file_sharing()` — placeholder panel inside the real shared shell
   (sidebar + main panel + overlays), built from design tokens
   (`bg_surface`, `border_muted`, `text_primary`, `text_muted`, TYPO sizes,
   SPACE rhythm) and the approved `Icon::Files` Lucide glyph. No dashboard
   cards, tables, or backend projections on this card.
4. FILES sidebar section — a collapsible section header plus a single
   "File Sharing" navigation row, styled active (selected background +
   1 px primary border) while `Screen::FileSharing` is active. Added as the
   last section so every pre-existing section index and route is unchanged
   (chats/groups/friends/discover/public rooms/requests all preserved).
5. MCP/GUI-test surface: `GuiTestCommand::OpenFileSharing` with
   validate()/expected_state(), `gui_navigation_message` mapping, and the
   pending-action completion wiring so `boru_send_gui_action` with
   `{"command":"open_file_sharing"}` completes through the real update path.

## TESTS (all green)

- `gui_open_file_sharing_uses_file_sharing_navigation_message`
- `gui_navigation_mapping_includes_home_friends_settings_and_file_sharing`
- `file_sharing_navigation_preserves_persistent_shell_state`
  (Home → Files → ChatList → Files preserves account identity, sidebar
  collapsed state, and conversation-store count)
- `gui_navigation_actions_reach_completed_via_normal_update_path` (extended
  with the OpenFileSharing case)
- `cargo check --features gui --bin boru` → exit 0
- `cargo test --lib diagnostics::tests` → 234 passed

## RUNTIME / MCP EVIDENCE

App launched under Xvfb with `--mcp --enable-gui-test-actions --mcp-bind
127.0.0.1:18766 --no-dht --no-relay`, then driven over loopback JSON-RPC:

- `boru_ping` → `{"pong": true}`
- `boru_send_gui_action {"command":{"command":"open_file_sharing"}}` → sent;
  `boru_gui_get_action_status` → `state: "completed"`, expected_state
  `screen_is: "FileSharing"`
- Route cycle Home → Files → Chat → Files: `go_to_chat_list`,
  `open_file_sharing`, `open_room <topic>`, `open_file_sharing` all completed
  with their expected states; the final action reports
  `screen_is: "FileSharing"`.

## VISUAL EVIDENCE

- `files-screen-initial.png` — File Sharing placeholder inside the full shell
  (sidebar + header + placeholder card).
- `files-screen.png` — sidebar scrolled to show the FILES section with the
  "File Sharing" row selected (green background + primary border), with the
  placeholder in the main panel. All pre-existing sections
  (chats/groups/friends/discover/public rooms/requests) remain visible.

## SECURITY / PRIVACY IMPACT

No new pickers, protocols, storage, or network behavior. Native OS `rfd`
file selection is untouched; no in-app file browser was introduced. The GUI
test-action surface remains loopback-only and gated on
`--enable-gui-test-actions`.

## KNOWN LIMITATIONS / FOLLOW-UPS

- The placeholder panel is the FS-03 surface only; dashboard tabs, file
  table, and peer panels are later FS cards.
- The Home "Share files" quick action still dispatches `AttachPressed`
  (chat composer) per the FS-00 decision; retargeting it to the new Files
  route is a separate behavior decision.
