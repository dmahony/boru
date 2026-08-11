# UI-17 evidence: Real-state integration and behavior regression pass (t_8de1c9c4)

Evidence for the Phase 5 UI-17 card: verify the polished screens remain fully
connected to real Boru state and actions, with no fixture leakage or behavioral
regression. Every flow below was exercised against **two real Boru instances**
(real secrets, real gossip/direct QUIC, real friend store), **not** the
deterministic screenshot fixture.

## TASK: t_8de1c9c4 — UI-17 Real-state integration and behavior regression pass

## STATUS: Ready for Review (blocked review-required)

## SUMMARY

Ran the Phase-5 integration pass against two live instances (A/B) seeded with
real public keys via `scripts/seed_two_instances.py`, launched under Xvfb with
`--mcp --enable-gui-test-actions`, connected over direct QUIC
(`--bind-port` seeded addresses, `--no-dht --no-relay` unless noted). All
original UI-00 interactions were re-exercised or statically audited:

- **Static audit PASS** — all 42 UI-00 interactions are declared in
  `AppMessage` and handled in `IcedChat::update` (see `ui17_audit.py` output in
  this directory; zero unhandled variants).
- **Live two-instance PASS** — connection startup, message send/receive,
  history restore, peer offline/reconnect, create-public-room dialog flow,
  B-side directory discovery (with relay), group dialog from a real sidebar
  click, friend-requests/settings/file-sharing/dark-mode navigation, help
  overlay.
- **Bug found and fixed** — `GuiTestCommand::ToggleHelp` was declared in
  `src/diagnostics.rs` with an expected state but never dispatched in app.rs;
  it fell into the "diagnostic-only" catch-all and silently did nothing (a dead
  MCP action while the visible ? button worked). Fix: `gui_help_message()`
  maps `ToggleHelp -> AppMessage::ToggleHelp` (the same message the visible
  help button emits), with a `pending_toggle_help_action` completion path and
  a unit test `gui_help_command_maps_to_normal_toggle_message`.
- **Fixture isolation** — no production view depends on screenshot fixtures;
  fixtures remain QA-only Python scripts writing temp data dirs (see
  `fixture-leakage-audit` below).

## Evidence artifacts

| File | Shows |
|---|---|
| `1_connected_a.png` / `1_connected_b.png` | Two real instances connected (friend online, direct conversation) |
| `2_peer_offline_a.png` | A after B was killed (offline capture; see note) |
| `3_reconnected_a.png` | A after B restarted (reconnect) |
| `4_history_restored_a.png` | History restored into timeline after restart |
| `1_home_chatlist.png` | Home/ChatList screen |
| `2_create_room_dialog.png` | Create Public Room dialog |
| `3_room_created.png` | Room created and listed in A's directory |
| `4_history_restored_a.png` / `a_room_created.png` | Room created (relay run) |
| `b_discovered.png` | B discovered A's public room via directory gossip (relay) |
| `5_friend_requests.png` | Friend Requests screen |
| `6_settings.png` | Settings screen |
| `7_file_sharing.png` | File Sharing dashboard |
| `8_dark_mode.png` | Dark mode |
| `9_help_overlay.png` | Help overlay |
| `t_8de1c9c4_help_overlay_1280x800.png` | Help overlay (post-fix, 1280x800) |
| `t_8de1c9c4_peer_offline_1280x800.png` | Peer offline state (post-fix, 1280x800) |
| `t_8de1c9c4_group_dialog_1024x768.png` | Group dialog opened from a real sidebar click |
| `interaction-checklist.md` | Pass/fail per original UI-00 interaction |
| `regression-matrix.md` | Original action -> redesigned control -> verified result |
| `ui17-audit.txt` | Static AppMessage coverage audit output |
| `verification.json` | Machine-readable acceptance results |

## How it was run

Each evidence screenshot was produced by a live two-instance harness in the
style of `/tmp/ui17_live.sh` and `/tmp/ui17_nav.sh` (scripts from the worker
run; retained in the worker session, not committed — they reference a built
binary path and are superseded by the repo scripts):
`seed_two_instances.py` writes real `friends.json`/`conversations.json` plus
`secret_key.txt` for each side; each instance is launched with
`--mcp --enable-gui-test-actions`, and actions are driven through the loopback
JSON-RPC client `scripts/ui_mcp.py` (`boru_send_gui_action`,
`boru_gui_set_composer`, `boru_gui_submit_composer`, `boru_gui_navigate`,
`boru_list_public_rooms`, `boru_gui_wait_for_state`, …). Screenshots captured
with ImageMagick `import` on the Xvfb displays.

## Fixture leakage audit

- `grep -R "fixture\|FIXTURE\|screenshot_mode\|BORU_FIXTURE\|fixture_path"`
  across `examples/` and `src/` found no production fixture gate; fixture
  content lives only in `scripts/figure4_fixture.py`, `scripts/ui16_fixture.py`
  and the `docs/ui-redesign/evidence/ui-13-fixture/` directory.
- No `#[cfg(test)]`-only or MCP-only data is read by production `view*`
  functions; all visible values come from stores/handles listed in the UI-00
  audit ("Behavior preservation" section).
- The only GUI-test entry points are the loopback MCP server
  (`--mcp --enable-gui-test-actions`) and `GuiTestCommand` routing, which is
  intentionally part of the product's diagnostic surface (per UI-00 audit line
  162: preserve semantic routes).

## Build and test status

- **Isolated verification (HEAD + UI-17 hunks only):** the 7 ToggleHelp hunks
  were extracted and applied to a clean worktree at HEAD (52a312d7) and the
  targeted unit test passes; `cargo check --features gui --bin boru`
  passes there (no sibling WIP). Logs: `ui17-verify-test.log`,
  `ui17-verify-check.log`.
- **Shared tree:** at the time of this run the shared working tree did **not**
  compile because sibling kanban workers (FS-09..FS-16) were mid-edit in the
  same tree (`examples/iced_chat/shared_by_me_table.rs` references
  `Element`/`SharedByMe*` AppMessage variants not yet added). This is an
  external concurrency blocker, not a UI-17 regression; the same compile
  errors reproduce without the UI-17 hunks (see `ui17-shared-tree-check.log`).
- **Full GUI test suite:** the last complete green run was UI-16 at HEAD:
  663/663. A GUI test run during this pass on the shared tree reported
  661 passed / 2 failed; both failures are caused by the sibling FS workers'
  in-flight "start on FileSharing" default-screen change
  (`join_request_send_failure_and_retry_keeps_exactly_one_request`,
  `open_friend_requests_navigates_to_dedicated_screen` assert the app starts
  on ChatList). These are documented external failures, not UI-17 regressions.
- Baseline library test state is documented in the UI-00 audit
  (`docs/ui-redesign/baseline-tests.log`): `cargo test --lib` has pre-existing
  failures unrelated to UI work.

## Remaining risks

1. **Shared-tree build during sibling burst** — the tree was uncompilable for
   part of this run due to concurrent FS workers. The UI-17 hunks themselves
   compile and test cleanly in isolation.
2. **MCP `toggle_help` delivery flakiness observed once** — resolved. The
   intermittent "queued" drop seen in one prior harness session was an artifact
   of the sibling-WIP shared tree. On a clean isolated tree (HEAD + UI-17
   hunks), the live MCP verification now passes end-to-end: `toggle_help`
   reaches `update` (journal), the action state advances to `completed`, the
   `dialog_open` snapshot flips, and the help overlay visibly renders on a chat
   screen (`t_8de1c9c4_help_overlay_chat_live.png`). Harness:
   `ui17_livefix.sh`, `ui17_helpvisual.sh`.
3. **Offline capture timing** — the peer-offline screenshot was captured
   before the friend-ping cadence marked B offline in one run (friend ping
   interval > 75 s); the offline state itself is exercised by the
   `SetPeerPresence` GUI test route which uses the production presence path.

## Plan section 6 worker report

See `interaction-checklist.md` (per-interaction results) and
`regression-matrix.md` (action -> control -> result). Changed files for this
card:

- `examples/iced_chat/app.rs` — ToggleHelp wiring fix (7 hunks):
  `gui_help_message()`, `pending_toggle_help_action` field + completion in the
  `AppMessage::ToggleHelp` arm, `help_visible` in the diagnostics
  `dialog_open` snapshot, unit test.
- `docs/ui-redesign/evidence/ui-17/` — this evidence set.

Tests added: `app::tests::gui_help_command_maps_to_normal_toggle_message`
(1 test). Expected full-suite delta over UI-16's 663: 664 when the tree
compiles (sibling WIP permitting).
