# t_40d92fbe — UI-14 Message groups, bubbles, metadata and delivery state

Modern incoming/outgoing message presentation for the Boru chat timeline
(Phase 4, Figure 4). All changes are presentation-only: message records,
timestamps, sender fields, ordering and persistence are untouched.

## What changed

- **Incoming bubbles** — `surface` (white in light mode) with a subtle 1 px
  `border_muted` outline, 12 px radius (`RADIUS_LG`).
- **Outgoing bubbles** — `primary_soft` (#EAF5EE) soft green, 12 px radius,
  no border (surface contrast is sufficient).
- **Alignment** — incoming groups left, outgoing groups right. The avatar
  slot sits at the leading edge (left) for incoming and the trailing edge
  (right) for outgoing.
- **Avatar once per group** — the sender avatar renders on the FIRST bubble
  of each visual group only; later bubbles in the group reserve the same
  36 px (`AVATAR_SM`) slot so every bubble in the group shares one edge.
  Entries without a profile image get a coloured-circle fallback with the
  sender's initial instead of a bare `?`.
- **Max width** — `min(560 px, 68 % of the timeline width)` via
  `presentation::chat_bubble_max_width`, fed by the `responsive` wrapper in
  `view_chat_panel` (which previously passed only the height).
- **Group gaps** — 6 px (`SPACE_6`) between bubbles inside one sender group,
  18 px (`SPACE_18`) between groups, per plan §4.
- **Timestamps + delivery indicator** — directly below each bubble
  (`formatted_time` is now cached by `update_cache`; it was previously never
  computed, so timestamps never rendered). The last outgoing message of a
  group shows `time · <delivery label>`.
- **Failed state** — the metadata row shows `Failed`, the label carries the
  `✗` icon, and the bubble gets a `danger` border. Error is never
  communicated by colour alone.

## Grouping rule (worker handoff)

Two adjacent user messages share a visual group when all of:

1. both are the same kind (`Local` or `Remote`),
2. they have the same sender (`sender_key`; local entries with a missing key
   still group because they belong to the current user),
3. the timestamps differ by at most **5 minutes**
   (`presentation::MESSAGE_GROUP_WINDOW_MS = 5 * 60 * 1000`).

Grouping is purely presentational: stored timestamps, sender fields and
message order are never modified, and replayed history gets identical
grouping to live delivery because the rule lives in the presentation layer
(`presentation::continues_message_group`).

## Delivery state → indicator mapping (worker handoff)

`boru_core::chat_history::DeliveryState` → `presentation::delivery_label`:

| Model state | Metadata label | Notes |
|-------------|----------------|-------|
| `Queued`    | `Sending`      | composed, not yet accepted by transport |
| `Sent`      | `Sent`         | local transport accepted the broadcast |
| `Delivered` | `Delivered`    | peer confirmed receipt; **promoted to `Seen` by the existing seen-on-visibility rule while the conversation is on screen** (read receipts only when the chat is actually visible) |
| `Seen`      | `Read`         | peer/user viewed the message |
| `Failed`    | `Failed`       | permanent failure; label click retries (`RetryOutgoingMessage`) |

Remote (incoming) messages do not track delivery state in the model, so they
never show an indicator — no fabricated precision. The label row also shows
the existing icon (`✓`/`✓✓`/`✗`/…), so state is never colour-only.

## Evidence

- `ui14_states_1280x800.png`, `ui14_states_1024x720.png` — bottom-anchored
  states timeline: emoji-only bubble, Queued→`Sending`, Sent→`Sent`,
  Delivered→`Read` (promoted on visibility), Failed→`Failed` with danger
  border and `✗` label, long unbroken word, multiline, long wrapped text.
- `ui14_states_top_1280x800.png`, `ui14_states_top_1024x720.png` — scrolled
  to the top of the same timeline (Read / Sent / emoji / system chips).
- `ui14-states-spec.json` — deterministic QA timeline (dev/QA only, isolated
  data dirs via `scripts/figure4_fixture.py --spec`).
- Figure 4 fixture rebaselined to the UI-14 design:
  `docs/ui-redesign/evidence/ui-13-fixture/figure4-baseline-{1280x800,1024x720}.png`
  with `figure4-comparison.json` (ok=true, 0 mismatched pixels) and
  `figure4-comparison-1024x720.json`.

## Verification (section 6 report)

- **Build:** `cargo build --features gui --example boru` — PASS
- **Tests:** `cargo test --features gui --example boru` — 652 passed / 0
  failed. New tests: `chat_bubble_max_width_caps_at_560_or_68_percent`
  (presentation), `bubble_bg_uses_spec_surfaces`,
  `bubble_border_follows_spec_rules` (design_tokens),
  `update_cache_formats_timestamp_once` (app).
- **Formatting/lint:** touched files are rustfmt-clean (pre-existing `cargo
  fmt` drift in `app.rs` elsewhere predates this card); `git diff --check`
  clean on the commit.
- **Interactions preserved:** click-to-copy (`CopyMessage`), right-click
  context menu (`RightClickText`), URL links, link previews, images/GIFs,
  reactions, retry-on-failed — all renderer paths unchanged, only styling /
  alignment / avatar grouping modified.
- **Screenshot command:** `scripts/ui13_visual_regression.sh`
  (figure4 fixture, `BORU_WIDTH`/`BORU_HEIGHT`) and
  `scripts/ui14_states_evidence.sh` (states timeline).

## Acceptance criteria

- [x] Conversation matches the Figure 4 bubble hierarchy — verified by
  vision review of `figure4-current-1280x800.png` (white/left incoming,
  soft-green/right outgoing, avatar once per group, tighter intra-group
  gaps).
- [x] All existing message interactions remain available — code paths
  untouched (see above).
- [x] Long content wraps without expanding beyond the timeline — long
  unbroken word and long wrapped sentence verified in `ui14_states_*`.
- [x] Delivery/read states are truthful and accessible — model mapping
  above; label text + icon + border, never colour-only.

## Remaining risks

- Evidence captured in light theme only (the redesign target); dark-mode
  bubble colours come from the existing theme-aware tokens
  (`primary_soft`/`surface` dark variants) but were not screenshot-verified.
- Headless captures cannot show a live peer session; evidence uses the
  sanctioned deterministic fixture (dev/QA only, isolated data dirs).
- The `Delivered` state is transient in an open conversation (promoted to
  `Seen` by the pre-existing seen-on-visibility rule), so screenshots show
  it as `Read`; the mapping table documents this.
