# UI-10 evidence: Online Peers, Recent Activity, and Tunnels

## Captures

- `t_42abbf42_empty_1280x800.png` — empty rail at the wide target window.
- `t_42abbf42_empty_600x720.png` — compact responsive rail capture.

The wide capture visibly verifies the three bounded right-rail cards:

- Online Peers: live online-presence count badge, total friend count, and truthful empty state.
- Recent Activity: live event count badge and truthful empty state.
- Tunnels: live tunnel count badge, truthful empty state, and the existing Manage action wired to `ShowCreateTunnelDialog`.

## State and overflow notes

- Peer rows are derived from `FriendsStore` plus `peer_presence`; the `Msg` control remains an accessible button dispatching `OpenConversation`.
- Activity rows are derived from `recent_activity`, use the event timestamp for relative time, and render at most 15 rows inside a fixed-height scrollable region. The ring buffer retains at most 50 events.
- Tunnel rows are derived from `TunnelService::list_tunnels`, show backend status/expiry labels, retain the existing close action, and render inside a fixed-height scrollable region.
- Activity events currently have a timestamp but no independent stable ID; the UI does not use identity-based actions for them, so it safely renders them as read-only rows. Peer public keys and tunnel IDs are stable identifiers for their actions.
- No fabricated peer, activity, or tunnel data is used in the production render path.

## Verification

- `cargo fmt --check` passed after formatting.
- `cargo check --features gui --example boru` passed.
- `cargo test --features gui --example boru` passed: 558 tests.
- `git diff --check` passed.

The captures exercise the real initializing/empty state. A live multi-peer populated fixture was not available in the isolated screenshot run; overflow is implemented with bounded scrollables and the activity 15-row display cap rather than an unbounded dashboard.
