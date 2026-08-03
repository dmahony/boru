# UI Online Peers card evidence (t_d4ca2ca4)

Home right-rail "Online Peers" card implemented with the reusable card shell
(`examples/iced_chat/card_shell.rs`). The card consumes real friend + presence
state only — no sample data in the render path.

## What the card provides

- Header: uppercase "ONLINE PEERS" title, count badge showing **online/total**
  (e.g. `8/8`, `0/0`) via the shell's `count` + `count_total` pair, and an
  optional "View all" header action wired to the existing friends-management
  navigation (`OpenFriendRequests`).
- Rows: compact 48 px rows (shared `CARD_ROW_HEIGHT`) with a peer avatar
  (profile image when a cached handle exists, otherwise initials / fallback
  icon) with a green online dot, plus the resolved display name (friend label →
  announced name → session name → short key).
- Row action: the entire row is the open-chat action (`OpenConversation(peer)`),
  preserving the previous UI's message control as a full-width accessible
  button with hover surface.
- Empty state: the shell's UI-04 empty-state typography renders
  "No peers online".
- Overflow: the shell's bounded body is sized to show exactly five 48 px rows
  (`5 * CARD_ROW_HEIGHT + 4 * SPACE_2`); the 6th peer scrolls.

## Captures

- `t_d4ca2ca4_empty_1280x800.png` — Home at the wide target. Right rail shows
  the Online Peers card with `0/0` badge, "View all" header action, and the
  truthful "No peers online" empty state.
- `t_d4ca2ca4_populated_1280x800.png` — Home with 8 seeded friends marked
  online through the production friend-status path
  (`boru_gui_set_peer_presence`, same `FriendEvent::StatusChanged` route real
  network events use). Badge reads `8/8`; the bounded body shows five rows and
  the 6th scrolls.
- `t_d4ca2ca4_zoom_1280x800.png` — zoomed crop of the populated Online Peers
  card: header badge `8/8` + "View all", five visible 48 px rows with avatars /
  green dots, and the bounded scrollable body.

## Verification

- `cargo check --features gui --example boru` — PASS.
- `cargo test --features gui --example boru` — 589 passed, 0 failed (3 new
  card-shell `count_total` unit tests + 2 new home-view smoke tests: empty and
  >5-peer populated Online Peers card builds).
- `cargo fmt --all -- --check` — PASS.
- `git diff --check` — PASS.
- Scrollbar pixel-verified in the populated capture at x≈1232–1236
  (#DEDEDE rail, #BDBDBD thumb — iced 0.14 default scrollable style, same as
  the parent card-shell evidence), confirming the bounded overflow behaviour.

## How to re-run

```bash
cargo build --features gui --example boru
bash scripts/ui_online_peers_evidence.sh
```
