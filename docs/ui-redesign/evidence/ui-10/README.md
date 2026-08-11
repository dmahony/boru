# UI-10 evidence: Online Peers, Recent Activity, and Tunnels

Root-card evidence for the Figure 3 right-rail cards. The rail is built from
real app state only (no sample data in the render path):

- Online Peers rows come from `FriendsStore` + `peer_presence_map` (the
  production presence map; `handle_friend_event` is the single mutation path).
- Recent Activity rows come from the in-memory `recent_activity` ring buffer
  (capped at 50 events, newest first; at most 15 rows rendered).
- Tunnels rows come from `TunnelService::list_tunnels()` (live in-memory
  registry snapshot; empty in isolated runs because a tunnel needs a real
  friend + created tunnel — accepted in t_5f03f97d).

## Captures

### Empty / initializing (from the initial implementation pass)

- `t_42abbf42_empty_1280x800.png` — empty rail at the wide target window.
- `t_42abbf42_empty_600x720.png` — compact responsive rail capture.

Wide empty capture verifies the three bounded cards: `ONLINE PEERS 0/0`
+ `No peers online`, `RECENT ACTIVITY (0)` + `No recent activity`, and
`TUNNELS (0)` + `No active tunnels`.

### Populated (16 friends, all online) — closes reviewer gap 1 & 2

- `t_42abbf42_populated_1280x800.png` — Home with 16 seeded friends marked
  online through the production friend-status path
  (`boru_gui_set_peer_presence`, same `FriendEvent::StatusChanged` route real
  network events use). OCR confirms `ONLINE PEERS | 16/16`, `RECENT ACTIVITY`
  badge with the 16 real `Peer NN came online` events (plus genuine app
  events for the isolated run's discovered peers), and `TUNNELS (0)`.
- `t_42abbf42_populated_600x720.png` — compact responsive rail with the same
  16-peer populated state.
- `t_42abbf42_zoom_1280x800.png` — zoomed crop of the populated rail: all
  three cards, 16/16 badge, bounded card bodies.

Overflow behavior (15+ peers/activities requirement): the Online Peers body
is sized to five 48 px rows (`5 * CARD_ROW_HEIGHT + 4 * SPACE_2`), so rows
6–16 scroll inside the fixed-height card; Recent Activity renders at most 15
rows in a fixed 180 px scrollable (the count badge still shows the full
buffer length). Neither card grows the dashboard.

### Live online → offline transition — closes reviewer gap 3

- `t_42abbf42_live_before_1280x800.png` — same 16-peer populated state,
  badge `16/16`.
- `t_42abbf42_live_after_1280x800.png` — three peers (14/15/16) driven
  offline via the same production presence path; OCR confirms the badge drops
  to `13/16` and Recent Activity gains `Peer 16 went offline`, `Peer 15 went
  offline`, `Peer 14 went offline` rows. The card rebuilt from real state in
  the same running app instance.

## State and overflow notes

- Peer rows derive from `FriendsStore` plus `peer_presence`; the full row is
  an accessible button dispatching `OpenConversation` (the preserved
  open-chat action). The `View all` header action dispatches
  `OpenFriendRequests`.
- Activity rows use the event timestamp for relative time (recomputed on the
  1 Hz `ActivityTick`), render at most 15 rows, and are read-only.
- Tunnel rows show backend status/expiry labels and retain the per-row close
  action (`CloseTunnel`); the `View all`/Manage header action dispatches
  `ShowCreateTunnelDialog` (click-through verified in t_5f03f97d).
- Fine-grained `iced::lazy` selectors (t_9aaac275) ensure a presence change
  rebuilds only the Online Peers card, an activity push only Recent Activity,
  and a tunnel status change only Tunnels — the per-second `ActivityTick`
  deliberately excludes the peers card so idle ticks do not rebuild it.

## Data notes (worker handoff)

- `RecentActivityEvent` has a `SystemTime` timestamp but **no stable ID**;
  rows are read-only display entries, so no identity-based action is needed
  and the missing ID is safe. The ring buffer is the only producer.
- Peer public keys and tunnel IDs are stable identifiers used for their
  respective actions (open chat / close tunnel).
- Fixture keys are genuine Ed25519 public keys (iroh rejects arbitrary hex
  strings that do not decode to a valid point); the friends are marked online
  exclusively through the production `FriendEvent::StatusChanged` path.

## Verification

- `cargo fmt --all -- --check` passed (one pre-existing one-line closure
  formatting drift in the chat view, committed by UI-12, was formatted).
- `cargo check --features gui --bin boru` passed.
- `cargo test --features gui --bin boru` passed: 596 tests, 0 failed.
- `git diff --check` passed.

## How to re-run

```bash
cargo build --features gui --bin boru
bash scripts/ui10_rail_evidence.sh
```
