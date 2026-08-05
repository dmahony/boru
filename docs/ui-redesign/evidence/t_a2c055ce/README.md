# UI evidence: Recent Activity refinement + Tunnels card (t_a2c055ce)

UI-HOME-08 evidence captured with `scripts/ui_home08_evidence.sh` under
Xvfb at 1280x800 (fresh data dirs, `--no-dht --no-relay`). No sample UI
content — every row is live app state.

## Captures

- `t_a2c055ce_home_empty_1280x800.png` — truthful fresh-launch home. OCR of
  the right rail: `ONLINE PEERS 0/0 View all / No peers online`,
  `RECENT ACTIVITY 0 / No recent activity`, `TUNNELS 0 View all /
  No active tunnels. Create or join a tunnel to securely route traffic.`
  (the spec empty copy).
- `t_a2c055ce_home_populated_1280x800.png` — seeded fixture + real
  friend-status events routed through the production
  `handle_friend_event -> push_activity` path (`has_been_seen` is true for
  seeded friends). OCR shows `ONLINE PEERS 2/3`, `RECENT ACTIVITY 5` with
  rows such as the truncated long-name peer and "Bob went offline" /
  "Alice came online" with right-aligned relative timestamps.
- `t_a2c055ce_activity_populated_card_1280x800.png` — zoomed Recent
  Activity card crop: `RECENT ACTIVITY 5`, per-row icons, the long name
  truncated with an ellipsis, and "just now" timestamps at the right edge
  with no description/timestamp collision (the description is clipped to
  the row's remaining width via a fill + clip container).
- `t_a2c055ce_tunnels_empty_card_1280x800.png` — zoomed Tunnels card crop:
  `TUNNELS 0 View all` + the full "No active tunnels. Create or join a
  tunnel to securely route traffic." empty copy.
- `t_a2c055ce_tunnels_viewall_after_1280x800.png` — click-through of the
  Tunnels card's "View all" header action: it opens the Create Tunnel
  dialog (OCR: "Create Tunnel", "Securely route traffic between peers.",
  "Choose a friend who will be able to connect through this tunnel.").
  The action is backed by a real destination (`ShowCreateTunnelDialog`).
- `t_a2c055ce_activity_populated_zoom_1280x800.png` /
  `t_a2c055ce_tunnels_empty_zoom_1280x800.png` — word-box crops used to
  verify the card headers via OCR.

## Data notes

- Recent Activity rows: `self.recent_activity` 50-event ring buffer
  (`push_activity`); the populated capture's events come from
  `boru_gui_set_peer_presence` MCP actions through the production
  `handle_friend_event` path. Relative timestamps use
  `presentation::relative_time_from_system` and refresh on the per-second
  `ActivityTick`.
- Tunnels rows: `TunnelService::list_tunnels()` + `shared_tunnels` name
  map. A clean launch truthfully shows zero tunnels; the live-row
  projection is covered by the app test
  `tunnels_card_projects_live_tunnel_row_and_renders` (registers a tunnel
  and asserts name/endpoint/status rendering).

## Re-run

```bash
bash scripts/ui_home08_evidence.sh
```
