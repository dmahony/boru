# Recent Activity card evidence (t_8a9f9181)

## Captures

- `t_8a9f9181_activity_empty_1280x800.png` — truthful empty state at the wide target window.
- `t_8a9f9181_activity_populated_1280x800.png` — populated rail with real activity events.
- `t_8a9f9181_activity_populated_zoom_1280x800.png` — zoomed crop of the Recent Activity card.

## What the captures verify

The Recent Activity card is implemented with the reusable `CardShell` component
(`examples/iced_chat/card_shell.rs`) and consumes the existing in-memory
activity event stream (`IcedChat::recent_activity`, a ring buffer capped at 50
events, newest first). No sample entries are rendered — every row is a real
event pushed through `push_activity`.

- **Empty state**: "RECENT ACTIVITY (0)" header badge plus the card-shell
  empty message "No recent activity".
- **Populated state**: "RECENT ACTIVITY (6)" with rows such as "Alice came
  online", "Bob went offline" and the seeded long-name peer. The events were
  produced by routing `boru_gui_set_peer_presence` through the production
  `handle_friend_event` → `push_activity` path (`has_been_seen` is true for
  seeded friends), so the screenshot shows genuine app state, not injected UI.
- **Row anatomy** (per task spec):
  - action-specific icon: Online → green presence icon, Offline / FileShared /
    Message / Generic → muted icon;
  - relative timestamp via the shared `presentation::relative_time_from_system`
    utility ("just now" in the capture; recomputed on every 1 Hz
    `ConnMonitorTick` re-render, so labels update at a reasonable interval);
  - title text summarizing the event, truncated with an ellipsis — the seeded
    long-name peer ("a-very-long-display-name-for-truncation-test-peer-42 …")
    renders as a single 48 px row ending in `…`.
- **Row height**: every row uses `CARD_ROW_HEIGHT` (48 px) and `Wrapping::None`
  so long titles truncate instead of wrapping, matching the other rail cards.
- **Bounded list**: the shell's fixed-height scrollable (180 px) keeps a busy
  feed from growing the dashboard; at most 15 rows are rendered.

## Data notes

`RecentActivityEvent` has a `SystemTime` timestamp but no stable ID; the rows
are read-only display entries, so no identity-based action is needed. Real
event producers (friend status changes, neighbor presence flush) are the only
sources; the ring buffer evicts the oldest event beyond 50.

## Verification

- `cargo check --features gui --bin boru` passed.
- `cargo test --features gui --bin boru` passed: 584 tests.
- `presentation::tests` (23) and `card_shell::tests` (10) all pass.
- `git diff --check` passed.
- Note: `cargo fmt --all -- --check` currently reports one diff at
  `examples/iced_chat/app.rs:22163`, which is inside the Tunnels card rows
  written concurrently by sibling task t_5f03f97d in this shared workspace;
  the Recent Activity card region itself is rustfmt-clean.
