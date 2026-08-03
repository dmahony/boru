# Scroll behavior investigation (task t_c5ffbf7d)

Feeds implementation task **t_9e9a0fcd** ("Implement scroll behavior preservation and
jump-to-latest integration") for parent **t_6f308ca5** (UI-13 scroll preservation).

Investigated: 2026-08-03, against the working tree on `main` (commit 1241e0e2 + uncommitted
UI changes). Verified `cargo check --examples` passes and `cargo test --example boru --features gui`
baseline is green (597 tests, 4 layout-cache tests pass).

---

## 1. What exists today

| Behavior | Status | Code location |
|---|---|---|
| Scroll-to-bottom when user is at bottom | Implemented (follow-latest + snap task) | `examples/iced_chat/app.rs` — see §2 |
| Unread anchor in the timeline | **Does not exist** | no unread marker / "new messages" divider anywhere |
| History pagination trigger in the GUI | **Does not exist** | history loads wholesale on open; backfill is network-driven (§4) |
| Jump-to-latest control | **Does not exist** | zero matches for jump/chevron/arrow-down button or shortcut |

The GUI chat log is a **single top-anchored `scrollable`** with id `CHAT_LOG`
(`app.rs:289-290`) that uses **windowed rendering**: only the entries intersecting the
current viewport are built as widgets, with spacers above/below so the scrollbar geometry
stays correct.

## 2. Scroll-to-bottom (user at bottom)

### State (app.rs)
- `follow_latest: bool` — `app.rs:2819-2820`, init `true` (`app.rs:5373`)
- `total_content_height: Cell<f32>` — `app.rs:2821-2822`, set every frame in `view_chat_log`
  (`app.rs:24661` empty, `app.rs:24682` non-empty)
- `scroll_offset: f32` / `viewport_height: f32` — `app.rs:2874-2875`, init
  `f32::MAX` / `0.0` (`app.rs:5375-5376`). `f32::MAX` is the **bottom sentinel**
  (`app.rs:4741-4745` in `LayoutCache::window`)
- `scroll_to_bottom_pending: bool` — `app.rs:2876-2880`, init `false`

### Mechanism
1. `Scrolled(offset, vp_h)` message (`app.rs:4236-4238`) is emitted by both chat-log
   scrollables via `.on_scroll(|v| AppMessage::Scrolled(v.absolute_offset().y, v.bounds().height))`
   — empty state `app.rs:24675-24677`, non-empty `app.rs:25395-25397`.
2. Handler `app.rs:16737-16756`: stores offset/viewport, then
   - `offset + vp_h >= total - 10` → `follow_latest = true` (at bottom)
   - otherwise → `follow_latest = false` **and cancels any queued snap**
     (`scroll_to_bottom_pending = false`, `app.rs:16748-16751` — uncommitted change)
3. Every entry append goes through `entries_push` (`app.rs:6571-6624`), which calls
   `keep_latest_visible()` (`app.rs:5759-5772`): if `follow_latest`, sets
   `scroll_offset = f32::MAX` and arms `scroll_to_bottom_pending = true`.
4. At the end of `update()` (`app.rs:17477-17488`), a pending flag is consumed into
   `iced::widget::operation::snap_to_end(CHAT_LOG)` — the task that actually drives the
   Iced scrollable to the bottom (the timeline is top-anchored, so content growth alone
   cannot move the viewport). **This snap consumption is an uncommitted change.**
5. Per-conversation persistence: `leave_current_room` saves `follow_latest`,
   `scroll_offset`, `viewport_height` into the `ConversationLive` map
   (`app.rs:7280-7282`); `switch_to_conversation` restores them and re-arms the snap when
   `follow_latest` (`app.rs:7403-7411`).

### Conversation-open flows
- **Fast path** (already-subscribed conversation): `OpenRoom` → `switch_to_conversation`
  (`app.rs:8880`), restores scroll + snap (above).
- **Slow path** (first-time subscription): `OpenRoom` (`app.rs:8838+`) → `leave_current_room`
  → async subscribe → `RoomOpened` (`app.rs:9216-9729`). `RoomOpened` replays history into
  `self.entries` (`app.rs:9362-9397` then again `app.rs:9479-9568`), pushes system messages
  (`app.rs:9464-9465`), defers backfill (`app.rs:9632-9648`), then switches to
  `Screen::Chat`. It does **not** explicitly arm `scroll_to_bottom_pending`; the only arm
  is indirect via `entries_push → keep_latest_visible`, which requires `follow_latest`
  to still be true.

### Known gap (observed failure)
The probe scripts (`scripts/scroll_roundtrip_probe.sh`, `scripts/scroll_probe.sh`) recorded
**state 1 FAIL on every run** at 20:02–20:27 (e.g. `/tmp/scroll-roundtrip-run4.log`):
after opening a conversation seeded with 60 history messages, OCR shows the **top** of the
timeline ("Today", "Chat joined.", system chips) instead of the newest messages. Those runs
used a binary built **before** the uncommitted snap-consumption change
(`target/debug/examples/boru` was rebuilt 20:45, after the `app.rs` edit at 20:37), so the
current tree has **not** been probe-verified yet.

Likely failure chain to verify during implementation:
1. The empty/loading scrollable (`.anchor_bottom()`, `app.rs:24670-24677`) renders while
   `entries` is empty and `total_content_height` is 0.
2. Iced emits `Scrolled(0, vp_h)`; with `total == 0` the handler (`app.rs:16743-16746`)
   takes no branch — `follow_latest` stays `true` but **`scroll_offset` is clobbered from
   the `f32::MAX` sentinel to 0**.
3. `RoomOpened` then renders the **top window** (`LayoutCache::window` uses the real
   offset 0), and if the snap never fires (or fires then a later `Scrolled` cancels it),
   the user lands at the top of history instead of the bottom.

## 3. Unread anchor positioning

**No unread anchor exists in the timeline.** There is no unread separator, no "new
messages" divider, and no persisted unread position marker. What exists instead:

- Sidebar unread **count badge** per conversation (`conversation.unread`, cleared on view
  `app.rs:11592`).
- **Auto read receipts** are sent for incoming remote text messages **only while
  `follow_latest`** (i.e. the chat is visible at the bottom): `app.rs:17864-17870`.

The parent task's "unread anchor when loading history" therefore has **no existing
implementation to preserve** — if the requirement is an in-timeline unread marker, it must
be designed from scratch (there is no stored "last read message id" per conversation in the
GUI state today).

## 4. History pagination triggers

**No scroll-triggered pagination exists in the GUI.** History is loaded wholesale:

- `RoomOpened`: JSON store replay (`app.rs:9479-9502`) + SQLite outgoing replay
  (`app.rs:9508-9568`), oldest-first, into the in-memory `self.entries`.
- **Backfill** (`src/backfill.rs`) is network-driven, not scroll-driven:
  - `BACKFILL_TRIGGER_THRESHOLD = 20` (`backfill.rs:70`); triggers when a room has fewer
    entries at open (`app.rs:9639-9648`, deferred via `pending_backfill_topics`,
    `app.rs:3029-3034`).
  - Requests fire on `NeighborUp` (`app.rs:18696-18740`); results are injected as
    `ConversationNetEvent`s into the net channel (`app.rs:18895-18945`), i.e. they arrive
    through the normal message path.
- **Memory cap, not pagination**: `MAX_ENTRIES = 2000` (`app.rs:399`); `enforce_entry_cap`
  (`app.rs:6680-6691`) drains from the front once the cap is hit (entries already saved to
  history are dropped from memory only).
- Unrelated: the MCP snapshot API has an envelope `has_more` signal (`main.rs:906-911`) —
  notification API pagination, not the GUI timeline.

## 5. Jump-to-latest control

**Does not exist.** No UI button (no chevron-down / arrow / FAB), no keyboard shortcut, no
message wiring to `snap_to_end`. The only way to reach the bottom today is wheel-scrolling
down or sending/receiving a message while `follow_latest`.

Decision point for t_9e9a0fcd: the parent body says "add one only if explicitly in scope
(check investigation findings)". Finding: nothing exists, and the plan (UI-13) centers on
preserving behavior rather than adding new controls — adding a jump-to-latest button would
be **new scope**, not integration.

## 6. Existing tests

| Test | Location | Coverage |
|---|---|---|
| `layout_cache_remove_last_entry_rebuilds_without_panicking` | `app.rs:31751` | cache eviction |
| `layout_cache_remove_middle_entry_rebuilds_suffix` | `app.rs:31769` | cache eviction |
| `layout_cache_unchanged_entries_keep_cached_geometry` | `app.rs:31789` | cache stability |
| `layout_cache_tracks_multiple_image_entries_correctly` | `app.rs:32796` | image heights |
| `catalogue_window_calculation_chooses_visible_range` / `..._bottom_scroll_shows_last_files` | `app.rs:32910`, `app.rs:32936` | window math (inline copy, not the real `LayoutCache::window`) |
| `ChatLog` scroll_up/scroll_down/follow (terminal-style log) | `src/chat_core.rs:637-668`, tests `app_state_push_system_adds_entry_and_sets_follow` `chat_core.rs:2912` | separate line-based model, **not** the GUI |
| `stress_entry_scrolling` | `tests/stress_test_comprehensive.rs:167` | perf of entry iteration/height estimation |

**No unit test exercises the GUI scroll state machine**: `AppMessage::Scrolled` handler
transitions, `keep_latest_visible`, `scroll_to_bottom_pending` arming/cancelling, or the
snap task are untested at unit level. End-to-end verification is done via
`scripts/scroll_probe.sh` (single instance, seeded history) and
`scripts/scroll_roundtrip_probe.sh` (two real instances, live network appends) — both
produce OCR-checked PNG evidence; both last failed on state 1 against a stale binary.

## 7. Edge cases the implementation must handle

- **Empty → non-empty transition**: `Scrolled(0, vp)` from the anchor-bottom loading
  scrollable can clobber the `f32::MAX` sentinel before content exists (§2 gap). Consider
  ignoring `Scrolled` while `total_content_height == 0` or while `room_loading`.
- **Resize**: viewport height is re-measured via the `responsive` wrapper each frame
  (`app.rs:22489-22512`); `Scrolled` events re-fire. Bottom-anchored behavior must survive
  a resize without a phantom scrollbar (gap-overhead logic `app.rs:24704-24731`).
- **Rapid scrolling**: the `-10px` bottom epsilon (`app.rs:16744`) and the cancel-queued-
  snap on manual scroll (`app.rs:16748-16751`) must not fight the snap task.
- **Image hydration / height changes**: `keep_latest_visible` re-sets the sentinel so a
  mid-scroll image load cannot strand the viewport (`app.rs:5759-5772`); content growth
  while scrolled up must keep the reading position (top-anchored window keeps offset).
- **Entry-cap eviction** (`app.rs:6680-6691`): draining from the front shifts indices;
  `layout_cache` is invalidated, and `scroll_offset` semantics (top-relative) stay valid.
- **Conversation switches**: per-conversation `follow_latest`/`scroll_offset` persistence
  (`app.rs:7280-7282`, `app.rs:7403-7411`) must not be broken by new snap logic.

## 8. Recommended next steps for t_9e9a0fcd

1. Re-run `scripts/scroll_probe.sh` and `scripts/scroll_roundtrip_probe.sh` against the
   **current** binary (includes the uncommitted snap changes) to establish the real
   baseline; fix state 1 (fresh-open lands at top) if it still fails.
2. Decide the unread-anchor question: no existing anchor exists — either design a minimal
   one (needs a per-conversation last-read position) or document it as out of scope.
3. Decide jump-to-latest: not present; adding a button is new scope per the parent body.
4. Add unit tests for the `Scrolled` handler transitions and snap arming/cancelling —
   the state machine is currently untested.

---

## 9. Implementation (t_9e9a0fcd, completed)

Implemented 2026-08-03 in `examples/iced_chat/app.rs` (commit titled
"fix(t_9e9a0fcd): preserve chat scroll-to-bottom and reading position", see
`git log --oneline --grep=t_9e9a0fcd`). Summary of what changed and what was
deliberately NOT changed.

### 9.1 Code changes

- **`AppMessage::Scrolled` handler hardening** (`app.rs:16737`): while the timeline is
  empty (`total_content_height == 0`) the handler now only learns `viewport_height` and
  leaves `scroll_offset` untouched. Previously the anchor-bottom empty-state scrollable's
  `Scrolled(0, vp)` clobbered the `f32::MAX` bottom sentinel to `0`; combined with the
  snap task that was the suspect root cause of the "fresh conversation opens at the TOP
  of history" failure observed by the probes. With content present the handler mirrors
  the offset/viewport, sets `follow_latest` at the bottom (10px epsilon), and cancels any
  queued snap when the user scrolls away from the bottom (kept from the working tree).
- **Snap consumption** (`app.rs:17483`, kept from the working tree): a pending
  `scroll_to_bottom_pending` flag is converted into `iced::widget::operation::snap_to_end(CHAT_LOG)`
  exactly once per update — this is what actually drives the top-anchored scrollable to
  the latest entry (content growth alone cannot move the viewport).
- **No change** to `keep_latest_visible`, per-conversation `follow_latest`/`scroll_offset`
  persistence (`leave_current_room` / `switch_to_conversation`), `enforce_entry_cap`, or
  the backfill path.

### 9.2 Behaviors preserved / out of scope (decisions)

- **Scroll-to-bottom when user is at bottom**: implemented + verified (probe state 1/4/5).
- **Scrolled-up reading position not stolen by live appends**: implemented + verified
  (probe state 3; `keep_latest_visible` only arms the snap while `follow_latest`).
- **Unread anchor when loading history**: **out of scope** — no unread anchor exists in
  the timeline (no in-timeline marker or persisted last-read position; only the sidebar
  badge and follow-latest read receipts). Nothing existed to preserve, and designing a
  new anchor is new UI scope per parent t_6f308ca5.
- **History pagination triggers**: **unchanged** — the GUI has no scroll-triggered
  pagination; history loads wholesale on `RoomOpened` and backfill is network-driven
  (`src/backfill.rs`, `BACKFILL_TRIGGER_THRESHOLD=20`, fired on `NeighborUp`). No scroll
  code touches that path.
- **Jump-to-latest control**: **out of scope** — no control exists (no button/shortcut);
  adding one is new scope per the parent body ("add one only if explicitly in scope").
  The scroll-to-bottom path it would drive is already reachable (wheel down while
  `follow_latest` snaps to the newest entry).

### 9.3 Verification

- `cargo check --examples` OK.
- `cargo test --example boru --features gui`: **608 passed / 0 failed** (was 597 baseline;
  7 new scroll state-machine tests added in `app::tests`):
  - `chat_scroll_empty_timeline_preserves_bottom_sentinel` — the sentinel fix.
  - `chat_scroll_at_bottom_keeps_following_latest_and_mirrors_offset`
  - `chat_scroll_away_from_bottom_cancels_queued_snap`
  - `chat_scroll_bottom_detection_uses_ten_pixel_epsilon`
  - `keep_latest_visible_arms_snap_only_while_following`
  - `entries_push_arms_snap_only_while_following` (live append while reading keeps offset)
  - `update_tail_consumes_pending_snap_once`
- `scripts/scroll_probe.sh` (single instance, seeded 60-message history, OCR-verified):
  **5/5 PASS** — state 1 (fresh open at latest; the previously failing state) now passes.
- `scripts/scroll_roundtrip_probe.sh` (two instances): **cannot pass in this environment
  today** — both instances never form the gossip mesh (`boru_get_peer_status` reports the
  peer not Connected, so no messages are ever sent and the timeline stays empty). This is
  a **pre-existing infrastructure limitation** (identical failures in the 20:24 runs
  against the stale binary, before any of this task's changes; no run of this script has
  ever passed here) and is unrelated to scroll logic. The single-instance probe exercises
  the same scroll state machine (fresh-open-to-bottom, scrolled-up reading, live appends
  not stealing position, back-to-bottom, append-at-bottom snaps). The test task
  t_727c1d5e should treat the two-instance mesh as a prerequisite to fix separately if
  real-network round-trip evidence is required.

### 9.4 Follow-up: fast-path history replay (same task, uncommitted at 9.1–9.3)

Re-verification of the probe on 2026-08-03 (run 3712) exposed a **pre-existing bug that
made the probe flaky**: opening a conversation that had been auto-subscribed at startup
took the **fast path** (`switch_to_conversation`) which restores in-memory entries but
**never replays `chat_history.json`**, so a chat opened after restart showed an **empty
timeline** until a network backfill happened to arrive. In the probe, the lobby's
`RoomOpened` auto-subscribes every stored conversation + every friend's direct topic
within ~1s of boot, so the subsequent MCP open almost always hit the fast path; the
9.1–9.3 probe runs passed only when the slow path won the race.

Root cause: `AppMessage::BackgroundSubscribed` created an empty `ConversationLive` and
never loaded persisted history (the `history_loaded` field existed but was
`#[expect(dead_code)]`). This is user-visible: after restarting with stored
conversations, clicking a chat could show "No messages yet" until a peer connected.

Fix (in `app.rs`, `BackgroundSubscribed` handler, committed with this task):
- Replay `chat_history.json` entries for the topic into a **newly-created**
  conversation (`or_insert_with`), mirroring the `RoomOpened` JSON replay (including the
  `event_id != 0` guard).
- Call `ChatEntry::update_cache()` on each replayed entry — the windowed renderer draws
  the body from `parsed_segments`, and entries built by `history_entry_to_chat_entry`
  start with `None`; without this the bubbles render empty.
- Set `history_saved_count` (so `enforce_entry_cap` won't re-save) and arm the
  `f32::MAX` bottom sentinel so a fast-path open renders the newest messages
  immediately (consistent with `keep_latest_visible` on the slow path).
- Duplicate `BackgroundSubscribed` events (conversation-store loop + friends loop both
  subscribe the same direct topic) are guarded by `history_loaded` — no double replay.
- The SQLite outgoing delivery-state overlay that `RoomOpened` applies to the *active*
  room is deliberately **not** applied to background conversations (pre-existing
  behaviour; persisted delivery states in `chat_history.json` are used instead). If
  exact post-restart delivery indicators are required, that overlay is a candidate
  follow-up.

Verification after the fix:
- 610/610 example tests green (was 608; +2 new unit tests:
  `background_subscribed_replays_history_into_new_conversation` (includes
  `parsed_segments` non-None and duplicate-subscribe no-double-replay assertions) and
  `background_subscribed_without_history_creates_empty_conversation`).
- `scripts/scroll_probe.sh` re-run: **5/5 PASS, now deterministic** — state 1 shows
  `Seed msg 054-059` (fresh open at latest), state 2/3 show `041-046` (scrolled up and
  live appends do not move the reading position), state 4/5 show `055-060` (back to
  bottom and append-at-bottom snaps).

---

## 10. Test task t_727c1d5e (completed)

Comprehensive test pass over all five scroll behaviours in the task scope.
Committed as `test(t_727c1d5e): cover scroll state machine, unread badge, backfill gate; fix stale snap on conversation switch`.

### 10.1 New unit tests

In `examples/iced_chat/app.rs` (example suite, now **615 tests**; was 610):

| Test | Behaviour covered |
|---|---|
| `conversation_switch_at_bottom_rearms_snap` | Manual scroll-to-bottom trigger: opening a follow-latest conversation re-arms `snap_to_end` so the newest message shows immediately |
| `conversation_switch_preserves_scrolled_up_reading_position` | Scroll position preservation **across conversation switches**: reading at offset 500 in room A, switch to B and back → offset/viewport restored, no stale snap |
| `inactive_room_message_increments_unread_badge` | The only unread mechanism that exists (sidebar badge): user-visible message to a hidden room bumps `unread` and queues the event |
| `inactive_room_gossip_events_do_not_increment_unread` | NeighborUp/Presence noise never bumps the badge |
| `opening_conversation_clears_unread_badge_but_keeps_backlog` | Viewing clears the badge while the pending backlog is retained for incremental replay |

In `src/backfill.rs` (lib suite):

| Test | Behaviour covered |
|---|---|
| `try_backfill_skips_when_history_at_or_above_threshold` | History pagination is **network-driven, not scroll-driven**: at/above `BACKFILL_TRIGGER_THRESHOLD` no request is made; an unknown peer below threshold degrades to `Ok(None)` |

### 10.2 Regression found and fixed

`conversation_switch_preserves_scrolled_up_reading_position` initially **failed**,
exposing a real bug in the t_9e9a0fcd implementation: `switch_to_conversation`
only ever *set* `scroll_to_bottom_pending = true` (when the target follows latest)
and never cleared it. A snap armed while switching away from a follow-latest room
survived into the next conversation, so the queued `snap_to_end` fired at the end of
the update and **stole the reading position** of a scrolled-up room opened right
after. The `Scrolled`-handler cancel (e2a0a6ce) covered manual wheel scrolls but not
the conversation-switch path. Fix: recompute the transient flag from the restored
`follow_latest` state — `self.scroll_to_bottom_pending = self.follow_latest`
(`app.rs`, `switch_to_conversation`).

### 10.3 E2E probe is now self-asserting

`scripts/scroll_probe_check.py` parses the probe's five OCR dumps and asserts the
visible `Seed msg NNN` range per acceptance state; `scripts/scroll_probe.sh` runs it
at the end and exits non-zero on any failure. Two consecutive runs:
**5/5 PASS, identical maxima (60 / 48 / 48 / 60 / 60)** — fresh open lands at the
latest message, wheel-up shows older history, live appends while scrolled up do not
move the reading position, wheel-down returns to latest, and append-at-bottom snaps
to the newest entry.

### 10.4 Behaviours verified as not implemented (documented, per t_9e9a0fcd decisions)

- **Unread anchor in the timeline**: does not exist. Only the sidebar badge +
  follow-latest read receipts exist; those semantics are pinned by the new unread
  badge tests. An in-timeline anchor would need a per-conversation last-read
  position (new UI scope, per parent t_6f308ca5).
- **Jump-to-latest control**: does not exist (no button/shortcut; grep-verified).
  The scroll-to-bottom path it would drive is covered by the snap tests and probe
  state 5. Adding the control is new UI scope.
- **Scroll-triggered history pagination**: does not exist. History loads wholesale
  on open; backfill is network-driven and gated by `BACKFILL_TRIGGER_THRESHOLD`
  (now unit-pinned). No scroll code touches the backfill path.

### 10.5 Verification summary

- `cargo check --example boru --features gui` OK.
- `cargo test --example boru --features gui`: **615 passed / 0 failed** (was 610;
  +5 new tests). All 9 parent scroll tests still pass — no regressions.
- `cargo test --lib -- try_backfill_skips_when_history_at_or_above_threshold`: PASS
  (0.03s). The full lib suite was not re-run end-to-end: it is long-running
  (pre-existing; includes file-watcher/network tests) and the backfill edit is a
  self-contained test addition with no production-code change.
- `scripts/scroll_probe.sh`: **5/5 PASS twice**, now with automated OCR assertions.


