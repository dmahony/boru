# UI skeleton/loading treatment decision (t_0441a1dc)

**Decision: no skeleton/shimmer loading is added.** Confirmed against the
acceptance criteria ("confirmed no skeleton needed with rationale").

## Question

Does the initial load of the home right-rail cards — **Online Peers**,
**Recent Activity**, **Tunnels** — involve a meaningful asynchronous delay
(e.g. a network fetch of the peer list on mount) that a skeleton placeholder
should cover?

## Answer: No. All three data sources are synchronously available at first render.

| Card | Data source | When it is populated | Async? |
|---|---|---|---|
| Online Peers | `self.friends` (`FriendsStore`) + `self.peer_presence_map` | Friends loaded from SQLite/JSON **before** the Iced window opens (`main.rs:919-933`); presence map seeded in `IcedChat::new` from persisted friend status (`app.rs:4987-4992`) | No — sync read at first render |
| Recent Activity | `self.recent_activity` in-memory ring buffer | Empty at startup; appended synchronously by `push_activity` in message handlers (`app.rs:6411-6417`) | No — sync read at first render |
| Tunnels | `self.tunnel_service.list_tunnels()` | In-memory registry (RwLock-guarded map), created at startup (`main.rs:957`); rows read synchronously (`src/tunnel/service.rs:400-410`) | No — sync read at first render |

## Why a skeleton would be wrong here

1. **No mount-time fetch exists.** None of the three cards performs a network
   or disk read when the home view first renders. The peer list is loaded from
   disk *before* the GUI starts; presence, activity, and tunnels are all
   in-memory state.
2. **The only real async startup window is already covered.** Endpoint start,
   DHT registration, protocol handlers, and friend load all happen while the
   native splash window (`scripts/splash.py`, launched in `main.rs:620-666`)
   is visible. The Iced window — and therefore these cards — opens only after
   the backend is ready (`splash_send("Starting UI...")` / `"DONE"`,
   `main.rs:1347-1351`).
3. **Updates are event-driven, not fetch-driven.** Peer presence changes
   arrive as `FriendEvent::StatusChanged` messages; activity entries are
   pushed by app events; tunnels appear when created/received. Each message
   redraws the cards synchronously with real data. There is no intermediate
   "empty while fetching" state to fill.
4. **The task forbids artificial delay.** Showing shimmer for the sake of it
   would require faking an async loading phase (or delaying a sync read),
   which the task explicitly prohibits ("Do not add artificial loading
   delays").

## Progressive enhancement already present

Where genuinely async work exists (friend profile images re-queued for blob
download on startup, `app.rs:5096-5111`), the UI already handles it without a
skeleton: rows render immediately with initials/fallback avatar, and the
downloaded image replaces it when it arrives. This is the correct pattern —
the card body is never empty due to a pending fetch.

## Where the decision lives

- Code comment above the right-rail card construction (`app.rs`, "Right rail:
  loading treatment decision") so future UI workers see the rationale at the
  point of use.
- This document.

## Verification

- Source-level audit of `main.rs` (friend load before Iced window,
  `splash.py` lifecycle) and `app.rs` (presence seeding, activity ring
  buffer, tunnel service read) — see table above.
- `cargo check --features gui --example boru` — PASS.
- `cargo fmt --all -- --check` — PASS.
- `git diff --check` — PASS.
- No behavior change: comment-only diff. No skeleton code added, so no new
  tests are warranted; the existing home-view smoke tests
  (`home_online_peers_card_*`) and card-shell tests continue to cover the
  real-data render path.
