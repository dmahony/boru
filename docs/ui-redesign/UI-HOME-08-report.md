# UI-HOME-08 — Recent Activity refinement + Tunnels rail card

- Task: `t_a2c055ce` (UI-HOME-08)
- Repo: `/home/dan/iroh-gossip-chat` @ `main` (based on `d14d55e4` UI-HOME-03)
- Status: DONE (build green, 867/867 tests pass, screenshots + interaction verified, pushed)
- Labels: ui-home, design-system, visual-qa, regression-risk

## What changed

Only `examples/iced_chat/app.rs` (plus evidence + the capture harness). No
tunnel networking logic, no activity producers, no CardShell internals were
touched — the rail cards keep reading live state exactly as before.

### Recent Activity card (`view_recent_activity_card`)

- **Description/timestamp collision is now impossible.** The description
  previously sat in the row with `Wrapping::None` and no width bound; at the
  33% rail column (~250 px body at 1280×800) a 40-char event could overflow
  into the right-aligned relative timestamp. The description now renders
  inside `container(..).width(Fill).clip(true)` (the same clip pattern the
  File Sharing tables already use), so a long event is clipped at the row's
  remaining width and the timestamp always stays readable at the right edge.
  The 40-char `truncate_with_ellipsis` cap is kept as the first line of
  defence so typical long names still show a visible `…`.
- **Consistent row padding.** Activity rows now use the same `[0, SPACE_8]`
  horizontal inset as the Online Peers rows, so icons across the three rail
  cards share one left rhythm. Row height stays 32 px (UI-29 dense feed).
- **Icon treatment unchanged** (green presence dot for Online; muted
  relevant icon for Offline / FileShared / Message / Generic) — satisfies
  "small green dot or relevant icon" without inventing new glyphs.
- **Empty state** kept as the CardShell `empty_message` ("No recent
  activity"), extracted to `RECENT_ACTIVITY_EMPTY_MESSAGE` for the
  regression test. Final polish is explicitly owned by UI-HOME-16.

### Tunnels card (`view_tunnels_card`)

- Card is always present below Recent Activity in the right rail — never
  removed when empty (the empty CardShell body stays content-sized).
- Header shows the live count badge (`count(dep.rows.len())`) from
  `TunnelService::list_tunnels()` — real state, never sample data.
- Rows are live tunnels only: lock icon tinted by status, service name
  (shared-tunnel metadata → friend name → short key fallback), local target
  endpoint (`localhost:port`), truthful status label, per-row close action.
- **Empty copy updated to the spec sentence**: "No active tunnels. Create
  or join a tunnel to securely route traffic." (`TUNNELS_EMPTY_MESSAGE`).
- **"View all" action is backed by a real destination**: it dispatches the
  existing `AppMessage::ShowCreateTunnelDialog` (the same route the previous
  Manage button used) — verified by click-through (below). The action is
  intentionally kept in the empty state because the dialog is the "create or
  join a tunnel" destination the copy points at.

### Tests added (3)

- `home_rail_empty_state_copy_matches_ui_home_08_spec` — pins the exact
  Tunnels empty copy and the Recent Activity empty state.
- `tunnels_card_projects_live_tunnel_row_and_renders` — registers a real
  tunnel in `TunnelService`, asserts the card projects name
  ("Media Server"), endpoint ("localhost:8080"), status (Active), not
  expired, and that both the card and the full home screen render.
- `recent_activity_card_renders_long_description_rows` — pushes events of
  every `ActivityKind` including a >40-char description, asserts the
  selector passes the untruncated text (the view owns truncation + clip)
  and that the card + home screen render without panic.

## Verification

- `cargo build --example boru --features gui` — PASS (exit 0; 207
  pre-existing warnings, untouched).
- `cargo test --example boru --features gui` — **867 passed, 0 failed**
  (864 pre-existing + 3 new).
- `cargo fmt` — the rustfmt-normalized diff vs. the base commit contains
  only this card's semantic changes; the standing global fmt drift in
  app.rs (pre-existing, at shifted line numbers) is untouched, matching the
  UI-HOME-03 precedent.
- `git diff --check` — clean.

## Evidence (`docs/ui-redesign/evidence/t_a2c055ce/`)

Captured with `scripts/ui_home08_evidence.sh` (Xvfb, fresh data dirs,
`--no-dht --no-relay`, MCP GUI test actions; no sample UI content — every
row is live app state):

| File | What it proves |
| --- | --- |
| `t_a2c055ce_home_empty_1280x800.png` | Fresh launch: the full rail renders — `ONLINE PEERS 0/0`, `RECENT ACTIVITY 0 / No recent activity`, `TUNNELS 0 / No active tunnels. Create or join a tunnel to securely route traffic.` (OCR-verified) |
| `t_a2c055ce_home_populated_1280x800.png` | Seeded fixture + real `handle_friend_event → push_activity` events: `RECENT ACTIVITY 5` with rows such as the truncated long-name peer, "Bob went offline", "Alice came online", each with a right-aligned "just now" timestamp |
| `t_a2c055ce_activity_populated_card_1280x800.png` | Zoomed Recent Activity card crop: header + count, icon per row, truncated long name, timestamps right-aligned with no collision |
| `t_a2c055ce_tunnels_empty_card_1280x800.png` | Zoomed Tunnels card crop: `TUNNELS 0 View all` + full empty copy |
| `t_a2c055ce_tunnels_viewall_after_1280x800.png` | Click-through: clicking the Tunnels card's "View all" opens the Create Tunnel dialog ("Create Tunnel / Securely route traffic between peers. / Connection Target …") — the action is backed by a real destination |
| `t_a2c055ce_activity_populated_zoom_1280x800.png` | Word-box crop of the activity header |
| `t_a2c055ce_tunnels_empty_zoom_1280x800.png` | Word-box crop of the tunnels header |

### Data-source notes

- Recent Activity rows come exclusively from `self.recent_activity`, the
  50-event ring buffer pushed by `push_activity`; the seeded capture routes
  `boru_gui_set_peer_presence` through the production
  `handle_friend_event → push_activity` path (`has_been_seen` is true for
  seeded friends), so the rows are genuine app state. The seeded
  long-named peer ("a-very-long-display-name-for-truncation-test-peer-42")
  exercises the ellipsis truncation.
- Tunnels rows come exclusively from `TunnelService::list_tunnels()` plus
  the `shared_tunnels` name map; the fresh-launch capture truthfully shows
  zero tunnels. A populated tunnel row requires a real friend + created
  tunnel; the unit test `tunnels_card_projects_live_tunnel_row_and_renders`
  covers that projection with a registered live tunnel.
- Relative timestamps come from `presentation::relative_time_from_system`
  and refresh on the per-second `ActivityTick` (`activity_tick` is part of
  both card dependencies, so idle re-renders refresh "just now" → "1m ago"
  without touching the peers card).

## Interaction verification

1. Tunnels card "View all" → Create Tunnel dialog opens (OCR: "Create
   Tunnel", "Securely route traffic between peers.", Cancel present). The
   dialog is the same destination the previous Manage button used
   (`ShowCreateTunnelDialog`).
2. The dialog's Cancel closes it (script clicks Cancel after verifying).
3. Online Peers "View all" still routes to the friend-request screen
   (unchanged; exercised during click-debugging).

## Acceptance criteria

- Activity rows are aligned and readable — yes (consistent `[0,8]` inset,
  32 px rows, clipped fill description, right-aligned timestamp).
- Tunnels is always represented — yes (always in the rail; empty state
  content-sized; count badge from live service).
- No fake events or tunnels — yes (ring buffer + `TunnelService` only).
- Supported actions work — yes ("View all" → dialog verified; per-row
  close and peer rows unchanged).
- Right-column spacing is consistent — yes (SPACE_20 gaps unchanged from
  UI-HOME-02, three cards always present).

## Remaining risks / notes

- The standing global `cargo fmt` drift in app.rs (and several other files)
  predates this card and is intentionally left untouched; the 24610/24733
  fmt diffs reported against the current file are that pre-existing drift
  at shifted line numbers, not new.
- A populated tunnel ROW in a real screenshot still requires an actual
  tunnel (friend + share flow); the projection is covered by the new unit
  test. If a future card wants the visual, it needs a two-instance or
  seeded-tunnel harness.
- Empty-state final polish (illustration/refined copy) is deferred to
  UI-HOME-16 by the plan; this card ships the basic states.
