# UI-HOME-17 interaction + live-update verification evidence

All captures from the running Boru GUI under Xvfb (fresh data dir,
`--no-dht --no-relay`, MCP + GUI test actions enabled). Each action was
driven by a real pointer click or keyboard event against the rendered
window; every value shown is live app state.

Click targets are calibrated at runtime from the live capture via
`scripts/ui17_click_calibrate.py` (tesseract TSV word boxes) because the
home layout is content-driven and the quick-action grid shifts when the
mesh card height changes with the live event log.

| File | Proves |
| --- | --- |
| `home_1600x900_empty.png` | Home dashboard baseline (two seeded friends, both offline). |
| `action_1_create_public_room.png` | Clicking the Create Public Room quick action opens the redesigned Create Public Room dialog. |
| `action_2_create_group_chat.png` | Clicking Create Group Chat opens the redesigned Create Group Chat dialog. |
| `action_3_add_friend.png` | Clicking Add Friend navigates to the Friend Requests screen. |
| `action_4_share_files_root.png` | Share Files click dispatches AttachPressed; the native GTK picker is not renderable headless (root capture + app liveness). |
| `action_4_share_files_dashboard.png` | `boru_gui_test_share_file` drives the real SharedFilePicked → file-registration path (fixture filename visible in the Shared by Me table). |
| `rail_populated_1600x900.png` | Home with Ada online: peer row + populated rail. |
| `rail_5a_peer_row_chat.png` | Clicking an Online Peers row opens the Chat screen for that peer (OpenConversation preserved). |
| `rail_5b_online_peers_view_all.png` | Online Peers "View all" opens the Friend Requests screen. |
| `rail_5c_mesh_view_details.png` | Mesh Health "View details" opens the Connection Details dialog. |
| `rail_5d_tunnels_view_all.png` | Tunnels "Create tunnel" (empty state) opens the Create Tunnel dialog. |
| `live_before_1600x900.png` | Ada offline: Online Peers empty. |
| `live_after_online_1600x900.png` | Ada flipped online via the production friend-status path: row + "came online" activity. |
| `live_after_offline_1600x900.png` | Ada flipped offline: "went offline" activity appended. |
| `mesh_card_crop.png` | Mesh Health card crop: live status row, stat tiles, Recent events feed from the real bounded log. |
| `kb_ctrln_room_dialog.png` | Ctrl+N (global shortcut) opens the Create Room dialog without the mouse. |
| `kb_typed_name_autofocus.png` | The dialog auto-focuses the name input — typed text lands there. |
| `kb_tab_focus_order_group.png` | In the group dialog, Tab moves name → description (focus order intact). |
| `test_matrix.txt` / `ocr.txt` | Action-by-action matrix + per-capture OCR evidence. |
