# UI-HOME-16 empty-state evidence

All captures from the running Boru GUI under Xvfb with a fresh data dir,
`--no-dht --no-relay`, MCP-driven home navigation. Every value shown is
live app state — the empty states are the truthful fresh-launch state, not
sample content.

| File | What it proves |
| --- | --- |
| `t_4186e7f9_home_empty_1280x800.png` | Fresh launch: three rail cards show intentional empty states — Online Peers (0/0 badge, "No peers are online right now. Connected peers will appear here."), Recent Activity ("No recent activity. Network events will appear here."), Tunnels (0 badge, "Create tunnel" action, "No active tunnels. Create or join a tunnel to securely route traffic."). |
| `t_4186e7f9_online_peers_empty_1280x800.png` | Online Peers card crop: small muted icon + spec copy centred in the min-height body. |
| `t_4186e7f9_recent_activity_empty_1280x800.png` | Recent Activity card crop: small muted activity icon + spec copy. |
| `t_4186e7f9_tunnels_empty_1280x800.png` | Tunnels card crop: lock icon + spec copy + "Create tunnel" header action (the create/join dialog the copy points at). |
| `t_4186e7f9_mesh_events_empty_1280x800.png` | Mesh Health card with the live mesh log cleared via the test-only `boru_gui_clear_mesh_events` harness action: connection summary + stat tiles retained above, "No recent mesh events" below the divider. |
| `t_4186e7f9_mesh_events_crop_1280x800.png` | Zoomed "Recent events" crop of the above. |
| `t_4186e7f9_home_empty_800x600.png` | Minimum content band (top of page). |
| `t_4186e7f9_home_empty_800x600_scrolled.png` | Minimum content band scrolled: compact two-line card headers and the two-sentence copy wraps inside the narrow rail (no overflow — see ocr.txt geometry). |
| `t_4186e7f9_home_populated_1280x800.png` | Seeded fixture + real friend-status events: Online Peers gains rows and Recent Activity fills, proving the live transition out of the empty state. |

`ocr.txt` lists the tesseract verification per capture and the
right-edge overflow geometry (`words_past_right_edge=0` at every width).
The mesh no-events capture uses `boru_gui_clear_mesh_events`, a
`--enable-gui-test-actions`-gated harness tool that empties the live mesh
event log (it never fabricates events); the same state is reachable in
production when the watchdog purges transient startup lines after the mesh
goes Good.
