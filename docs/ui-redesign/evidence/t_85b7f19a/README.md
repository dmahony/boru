# UI-HOME-07 Online Peers card evidence (t_85b7f19a)

Captures of the refined Home right-rail "Online Peers" card (UI-HOME-07).
The card consumes real friend + presence state only — no sample data in the
render path. Presence is driven through the production friend-status path
(`boru_gui_set_peer_presence` → `FriendEvent::StatusChanged`, the same route
real network events use).

## What the card provides (after UI-HOME-07)

- Header: small-label uppercase "ONLINE PEERS" heading (CardShell), live
  online/known count badge (`1/1`, `0/0`), and a right-aligned "View all"
  header action wired to the existing friends-management navigation
  (`OpenFriendRequests`).
- Rows: structured two-line rows at 60 px (`PEER_ROW_HEIGHT`, plan band
  58–68 px) — peer avatar (profile image when cached, else initials/fallback
  icon) with a green online dot, resolved display name, and a live presence
  secondary line ("Online" green / "Away" amber / "Connecting…").
- Row action: the entire row is the open-chat action
  (`OpenConversation(peer)`) with a hover surface (iced 0.14 buttons have no
  `Focused` status, so hover is the interaction affordance).
- Content-driven body: grows with the row count up to five visible rows
  (the 6th peer scrolls) and never collapses below the 128 px floor, so a
  single peer keeps the card at a sensible ~224 px footprint instead of a
  tiny strip or a fixed 248 px blank panel.
- Empty state: "No peers online" centred in the min-height body (basic copy;
  final polish owned by UI-HOME-16).

## Captures (1280×800, fresh data dir, `--no-dht --no-relay`)

- `t_85b7f19a_empty_1280x800.png` — Home with no friends: `0/0` badge,
  "View all", truthful "No peers online" empty state.
- `t_85b7f19a_onepeer_1280x800.png` — Home with one seeded friend (Ada) marked
  online through the production path: badge `1/1`, one 60 px two-line row
  (avatar + online dot + "Ada" + "Online"), Recent Activity shows
  "Ada came online" (live state).
- `t_85b7f19a_onepeer_hover_1280x800.png` — same state with the cursor parked
  on the row: the row background shifts from white (255,255,255) to the
  `surface_hover` tint (239,243,241), pixel-verified.
- `t_85b7f19a_onepeer_viewall_1280x800.png` — after clicking "View all":
  navigates to the Friend Requests screen (interaction verified).
- `t_85b7f19a_onepeer_chat_1280x800.png` — after clicking the Ada row:
  navigates to the Chat screen for that peer (interaction verified).

## Verification

- `cargo build --example boru --features gui` — PASS.
- `cargo test --example boru --features gui` — 867 passed, 0 failed
  (3 new tests: peer-row-height band, body-height min/cap math, live
  presence in rows; existing empty/populated/dependency-isolation tests
  unchanged and green).
- Interaction clicks were calibrated against tesseract TSV word boxes from
  the first captures; final captures OCR-verified (empty state, `1/1`,
  "Online" presence, Friend Requests screen, Chat screen).

## How to re-run

```bash
cargo build --example boru --features gui
bash scripts/ui_home07_online_peers_evidence.sh
```
