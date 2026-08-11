# UI-HOME-17 — Home-screen interaction and live-update verification

- Task: `t_17a358c8` (UI-HOME-17)
- Repo: `iroh-gossip-chat` @ worktree `wt/t_17a358c8`
- Status: VERIFIED (build, 896 tests, action-by-action matrix, OCR-verified screenshots, keyboard/focus pass)
- Type: verification card — **no business logic changed**

## Summary

A complete functional and interaction verification pass over the redesigned
home screen (UI-HOME-04..16). Every home-screen action was exercised through
the production UI paths — real pointer clicks and keyboard events against the
rendered window under Xvfb, calibrated from the live capture via tesseract
word boxes — and every navigation target, live-state transition and keyboard
path was confirmed against the current code. All live data (peer counts,
presence, mesh status/events, recent activity, file registrations) flows from
the unchanged backend selectors; nothing is mocked. Build green, 896/896
tests pass, and the full action matrix is recorded below with per-action OCR
evidence.

## Verification setup

- `cargo build --bin boru --features gui` — **OK (exit 0; only the 207
  pre-existing warnings, untouched)**.
- `cargo test --bin boru --features gui` — **896 passed / 0 failed**.
- Evidence harness: `scripts/ui_home17_verification_evidence.sh` +
  `scripts/ui17_click_calibrate.py` (Xvfb, MCP, GUI test actions, tesseract
  TSV click calibration — same technique as UI-HOME-06/07/08).
- Fresh data dir seeded with two real friends (Ada, Bob, both offline),
  launched `--no-dht --no-relay --enable-gui-test-actions`; every value in the
  captures is live app state.

## Action-by-action test matrix (scope items 1–5, 7)

| # | Action (mouse) | Expected target | Evidence PNG | Result |
| --- | --- | --- | --- | --- |
| 1 | Create Public Room quick action | Create Public Room dialog (redesigned flow) | `action_1_create_public_room.png` | OK |
| 2 | Create Group Chat quick action | Create Group Chat dialog (redesigned flow) | `action_2_create_group_chat.png` | OK |
| 3 | Add Friend quick action | Friend Requests screen | `action_3_add_friend.png` | OK |
| 4 | Share Files quick action | AttachPressed dispatch → file-sharing flow | `action_4_share_files_root.png` + `action_4_share_files_dashboard.png` | OK |
| 5a | Online Peers peer row (Ada) | Chat screen for that peer (OpenConversation preserved) | `rail_5a_peer_row_chat.png` | OK |
| 5b | Online Peers "View all" | Friend Requests screen | `rail_5b_online_peers_view_all.png` | OK |
| 5c | Mesh Health "View details" | Connection Details dialog | `rail_5c_mesh_view_details.png` | OK |
| 5d | Tunnels "Create tunnel" (empty state) | Create Tunnel dialog | `rail_5d_tunnels_view_all.png` | OK |
| 5e | Recent Activity rows | Rendered live (no navigation links in the current design; rows are display-only by UI-HOME-08) | `rail_populated_1600x900.png` | OK |

| # | Action (keyboard) | Expected result | Evidence PNG | Result |
| --- | --- | --- | --- | --- |
| 7a | Ctrl+N (global shortcut) | Create Room dialog opens without the mouse | `kb_ctrln_room_dialog.png` | OK |
| 7b | Type after Ctrl+N | Dialog auto-focuses the name input — typed text lands in "Room name" | `kb_typed_name_autofocus.png` | OK |
| 7c | Tab in Create Group dialog | Focus moves name → description (typed text lands in Description) | `kb_tab_focus_order_group.png` | OK |

Independent OCR verification of every capture (crop + 2× upscale + tesseract)
was re-run during this verification pass and matches the matrix above:
- 1 → "Room name… / Visibility / Discovery / Advertise in Directory / Enable DHT discovery"
- 2 → "Create Group Chat / Start a private conversation with multiple selected peers."
- 3 → "Friend Requests / SEND A FRIEND REQUEST / Incoming Requests"
- 4 → "File Sharing / Shared by Me / hello-from-ui17.txt" (fixture registered)
- 5a → "End-to-end encrypted" (chat header for Ada)
- 5b → "Friend Requests / Back"
- 5c → "Local peer ID / Relay URL / Connected peers" (Connection Details)
- 5d → "Create Tunnel / Securely route traffic between peers. / Connection Target"
- 7a/7b → "Room Details / Room Name / Keyboard Room" (typed text in field)
- 7c → "Focus Group" (name) + "Focus Description" (description) after Tab

## Live-update verification (scope item 6)

Presence flips were driven through the production friend-status path
(`boru_gui_set_peer_presence` → `handle_friend_event → push_activity`), the
same path the live network uses:

| Step | Capture | Verified |
| --- | --- | --- |
| Ada offline (baseline) | `home_1600x900_empty.png` | ONLINE PEERS 0/2, empty state copy |
| Ada flipped online | `live_after_online_1600x900.png` | ONLINE PEERS **1/2**, Ada row visible, activity "Ada came online just now" appended |
| Ada flipped offline | `live_after_offline_1600x900.png` | ONLINE PEERS back to **0/2**, "Ada went offline just now" appended (log grows 4 → 5 → 6) |
| Mesh Health card | `mesh_card_crop.png` | Live status row ("Degraded — No peers in the mesh"), stat tiles 0/0/0, Recent events feed from the real bounded log |
| Share file registration | `action_4_share_files_dashboard.png` | Real `SharedFilePicked → file-registration` path; fixture filename visible in Shared by Me |

The peer-count badge, Online Peers rows, Recent Activity feed and Mesh Health
status/events all update live from the unchanged dependency slices; no
payload or state semantics changed.

## Scope-item status summary

1. Create Public Room → redesigned public-room flow — **verified** (dialog with Visibility/Discovery, Access/Participation, Preview sections).
2. Create Group Chat → redesigned group flow — **verified** (Group Details, participant search, Create Group).
3. Add Friend → correct flow — **verified** (Friend Requests screen).
4. Share Files → current file-sharing flow — **verified** (AttachPressed dispatch; native GTK picker is not renderable headless under Xvfb — same limitation documented by UI-HOME-06; the real file-registration path is verified via the MCP fixture and the dashboard table).
5. Online Peer rows / View all / Mesh View details / Tunnels action / Recent Activity — **verified** (matrix rows 5a–5e).
6. Live updates — **verified** (presence flips, badge counts, activity log, mesh status; all from real backend state).
7. Mouse interaction / keyboard activation / focus — **verified** (11 real clicks + Ctrl+N + autofocus + Tab order).
8. Unrelated backend defects — **none found** (app log during the entire evidence run contained only the expected libEGL/DRI3 warnings under Xvfb and the intentional `--enable-gui-test-actions` banner; no panics, no backend errors).

## Changed files (verification artifacts only)

- `docs/ui-redesign/UI-HOME-17-report.md` — this report
- `docs/ui-redesign/evidence/t_17a358c8/` — 19 PNGs + `README.md` + `ocr.txt` + `test_matrix.txt`
- `scripts/ui_home17_verification_evidence.sh` — evidence harness (new)
- `scripts/ui17_click_calibrate.py` — click calibrator (new)

**No business logic, payload, state or network behaviour was changed.**

## Acceptance criteria

- Every action reaches the correct existing flow — **yes** (matrix 1–5e, OCR-verified).
- Live status and lists continue to update — **yes** (peer count 0/2→1/2→0/2, activity log grows, mesh card live, file table populated).
- Keyboard and focus behaviour remain intact — **yes** (Ctrl+N, autofocus, Tab order).
- No payload or state semantics changed — **yes** (verification only; zero source changes).

## Remaining risks / notes

- Share Files' native GTK file picker cannot be screenshot-verified headless
  (Xvfb); the click dispatches AttachPressed (app stays alive) and the
  fixture-driven `boru_gui_test_share_file` path proves the downstream
  registration — the same boundary documented by UI-HOME-06.
- iced 0.14 buttons are not keyboard-focusable and `button::Status` has no
  `Focused` variant (pre-existing framework limitation, unchanged; hover is
  the interactive affordance). TextInputs do take focus, which is what the
  Tab-order and autofocus checks exercise.
- Recent Activity rows are display-only in the current design (no navigation
  links per UI-HOME-08); the scope item "links where present" therefore has
  no link targets to verify beyond the header actions already covered.
- The evidence run uses `--no-dht --no-relay` for determinism, so the mesh
  badge shows Degraded with real startup lobby lines; production values come
  from the same selectors.
- Pre-existing: ~207 build warnings, `cargo fmt` drift in app.rs (untouched,
  matches prior-card precedent).

## Downstream handoff

UI-HOME-18 (accessibility/visual/regression QA, `t_266bfba3`) and UI-HOME-19
(release gate, `t_61a56729`) both gate on this card. The action matrix is
complete, all interactions verified against live state, and the evidence set
is committed under `docs/ui-redesign/evidence/t_17a358c8/`.
