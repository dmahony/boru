# Home-rail card reactivity: fine-grained selectors + iced::lazy memoization

Task: t_9aaac275 — "Ensure reactive updates for peers and tunnels without
rebuilding other cards". The original ticket was written against React idioms
(`React.memo`, `useSelector` with shallow equality, React DevTools flamegraph).
This project is an **iced (Rust)** GUI, so the same goals are implemented with
iced's native memoization primitives. This document is the audit + fix record.

## 1. How iced renders (the "global re-render" model)

iced is retained-mode-ish: on **every** message, `IcedChat::view()` is invoked
and rebuilds the whole widget tree; iced diffs the old and new trees and only
re-lays-out / redraws changed regions. There is no per-component skip like
React.memo by default — a message anywhere in the app rebuilds the tree
(though the compositor redraw is limited to dirty regions).

Consequences found in the audit:

- The 1-second `ActivityTick` subscription (`main.rs:1588-1591`) fires forever,
  forcing a full `view()` rebuild every second even when the user is idle. This
  is intentional (relative timestamps must age) but previously rebuilt
  **everything**, including the Online Peers and Tunnels cards.
- Spinner animations (`SplashTick` at 100 ms while connecting/reconnecting)
  also rebuild the full tree at 10 fps.
- Each card's data was selected inline in `view_main_empty_state` by reaching
  into `self` for whatever it needed. Nothing stopped one card's data change
  from triggering full-tree reconstruction.

The tree is cheap to build (dozens of widgets), so this is not a hot-path bug
today — but it is exactly the class of "unnecessary re-render" the ticket asks
to eliminate, and it grows with card count.

## 2. Selector audit (what each card actually reads)

| Card | State fields read (pre-change) | Selector method |
|------|--------------------------------|-----------------|
| Online Peers | `friends` (labels/relationships), `peer_presence_map` (+`AWAY_THRESHOLD_MS`), `friend_image_handles`, `names` (via `resolve_name`), `dark_mode` | `IcedChat::online_peers_card_data()` |
| Recent Activity | `recent_activity` ring buffer (newest 15 shown, full length for the badge), `dark_mode` | `IcedChat::recent_activity_card_data()` |
| Tunnels | `tunnel_service.list_tunnels()`, `shared_tunnels` (service names), `names` (fallback labels), `dark_mode` | `IcedChat::tunnels_card_data()` |

No card reads another card's slice: the peers card never touches
`recent_activity` or the tunnel service, the activity card never touches
friends/presence, and the tunnels card never touches friends/activity.

## 3. Implementation

Each card now renders through `iced::widget::lazy(dep, build_fn)`:

```
let online_card  = iced::widget::lazy(self.online_peers_card_data(),   Self::view_online_peers_card);
let activity_card= iced::widget::lazy(self.recent_activity_card_data(), Self::view_recent_activity_card);
let tunnels_card = iced::widget::lazy(self.tunnels_card_data(),        Self::view_tunnels_card);
```

- `*_card_data()` are the **fine-grained selectors** (app.rs, "Home-rail card
  selectors" section). Each returns a `Clone + Hash + PartialEq` snapshot of
  exactly the slice its card renders:
  - `OnlinePeersCardData { dark_mode, total_friends, rows: Vec<OnlinePeerRow> }`
  - `RecentActivityCardData { dark_mode, tick, total, rows: Vec<ActivityRow> }`
  - `TunnelsCardData { dark_mode, tick, rows: Vec<TunnelRow> }`
- `iced::widget::lazy` compares the fresh snapshot with the previous frame's
  value (PartialEq) and **reuses the already-built subtree** when nothing in
  that slice changed — the iced equivalent of `React.memo` + a
  shallow-equality selector. A data change in one card rebuilds exactly that
  card; the other two subtrees are skipped.
- The build functions (`view_online_peers_card`, `view_recent_activity_card`,
  `view_tunnels_card`) are `fn(&Dep) -> Element<'static, AppMessage>` — they
  cannot reach other state by construction, which enforces the selector
  boundary at compile time.

### The tick

`AppMessage::ActivityTick` now bumps `IcedChat::activity_tick` (wrapping add).
The tick is included in the Recent Activity dependency (relative timestamps
must re-render every second) and the Tunnels dependency (an idle tunnel that
passes `expires_at_ms` flips to "Expired" within a second). It is deliberately
**excluded** from the Online Peers dependency — the peers card has no
time-dependent content, so idle ticks never rebuild it.

### Why Hash

iced 0.14's `lazy` requires `Dependency: Hash`. `TunnelStatus` does not
implement `Hash`, so `TunnelRow` implements `Hash` manually (hashing the
discriminant); `lazy`'s cache key uses the hash while the actual change
detection uses `PartialEq`.

## 4. Test harness (the flamegraph analogue)

There is no React DevTools in iced. The equivalent proof is a unit-test
harness in `app.rs` ("Home-rail card dependency isolation") that renders each
card's selector before/after a targeted mutation and asserts exactly one
dependency changed — because `lazy` rebuilds IFF the dependency changed, this
proves exactly one card re-renders:

| Test | Mutation | Asserts changed | Asserts unchanged |
|------|----------|-----------------|-------------------|
| `activity_push_changes_only_activity_card_data` | `push_activity` | Activity | Peers, Tunnels |
| `peer_presence_toggle_changes_only_peers_card_data` | friend presence map insert | Peers | Activity, Tunnels |
| `tunnel_status_change_changes_only_tunnels_card_data` | `connect_tunnel` → `mark_connected` → `mark_disconnected` | Tunnels | Peers, Activity |
| `activity_tick_refreshes_only_time_dependent_cards` | `update(ActivityTick)` | Activity + Tunnels (tick) | Peers |
| `lazy_card_dependencies_are_stable_without_change` | none | — | all three stable |

Run:

```
cargo test --features gui --bin boru -- card_data
cargo test --features gui --bin boru -- activity_tick_refreshes lazy_card_dependencies
```

Full suite: `cargo test --features gui --bin boru` → 596 passed, 0 failed.

## 5. Results vs acceptance criteria

- **Independent updates** — each card's dependency is its own slice; a change
  in one never changes another (proven by the harness).
- **No unnecessary re-renders** — with unchanged state, all three `lazy`
  subtrees are reused frame-over-frame; idle `ActivityTick`s rebuild only the
  time-dependent cards (Activity + Tunnels), never the peers card; peer or
  tunnel data changes rebuild only their own card.
- **Reported risks** — see below.

## 6. Remaining global re-render risks and fix plan (if deep)

Still present (by iced design, not a regression):

1. **Whole-tree rebuild per message.** Every message still calls `view()` and
   rebuilds the outer shell (sidebar, hero, action grid). `lazy` bounds the
   *card* work; the shell is still reconstructed each frame. The sidebar
   already uses the same revision-counter + lazy pattern, so the remaining
   un-memoized surface is the hero/connection card, action grid, and share
   strip — cheap today.
   Fix plan if it ever shows up in profiling: wrap the hero and action grid in
   `iced::widget::lazy` with their own fine-grained dependencies (connection
   variant, counts), and keep card data slices as the only thing each lazy
   reads.
2. **ActivityTick fires unconditionally.** The 1 Hz subscription is registered
   even when the Recent Activity card is not visible (e.g. inside a chat or on
   Settings). Scope it to `Screen::ChatList` (like `SplashTick` already is
   scoped) to stop idle ticks entirely off the home screen. Small, safe
   follow-up in `main.rs:1588-1591`.
3. **SplashTick at 100 ms** while connecting rebuilds the whole tree at 10 fps.
   Acceptable for a transient state; if it ever matters, gate the connecting
   spinner's frame counter to a lazy subtree so only the spinner redraws.
4. **`TunnelService::list_tunnels()` clones the tunnel map on every frame**
   (called from `tunnels_card_data`, which runs every `view()`). Bounded by
   `MAX_ACTIVE_SHARED_TUNNELS`; fine today. If tunnel count grows, add a
   `tunnels_revision` counter like the sidebar uses and cache the snapshot.
