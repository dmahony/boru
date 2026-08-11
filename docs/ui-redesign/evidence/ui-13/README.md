# t_affb6248 — UI-13 Chat timeline and system-event presentation

Timeline presentation for the Boru modern chat redesign (Phase 4, Figure 4):

- The message timeline is the sole expanding/scrollable region between the
  fixed conversation header and the pinned composer.
- Informational lines render as restrained centred system-event chips
  (muted surface, compact category label) instead of plain text rows.
- Consecutive plain system chips are grouped with tighter spacing than user
  messages (SPACE_2 vs SPACE_8) — see `ui-event-grouping/` for that
  evidence.
- Ordering is never changed: the render loop walks store order; the
  classification only selects a label + accent.

## Event variant selection

`boru_core::system_events::classify_system_event` (16 variants, total
mapping from t_85b9dbec) is the single source of truth. The presentation
layer maps each data-layer kind to a compact label + restrained accent via
`presentation::system_event_chip_meta` (JOIN/LEFT/NAME/FILE/HELP/ERROR/
NOTICE/WHISPER/INVITE/TUNNEL/TRANSFER/VIDEO/FRIEND/PROFILE/MESH/INFO).
Every entry is classified; nothing is silently discarded — entries without
a download attachment render as chips, download-bearing entries render as
their attachment card (both use the original body text).

## Evidence

- `t_affb6248_system_events_1280x800.png` — system-event-heavy timeline
  (membership + command-help notices) at the primary viewport.
- `t_affb6248_normal_1280x800.png` — normal conversation captured through
  the deterministic Figure 4 QA fixture: 7 user bubbles (incoming left /
  outgoing right), Read indicators, Today divider, INFO/NAME/LEFT chips.
- `t_affb6248_normal_1024x720.png` — same normal conversation at the
  alternate viewport.
- `t_affb6248_home_1280x800.png` — home/chat-list shell (context capture).

## Verification

- `cargo build --features gui --bin boru` — PASS
- `cargo test --features gui --bin boru` — 615 passed / 0 failed
- `git diff --check` — clean
- Timeline region geometry (`ui-timeline-region/verification.json`) — PASS
  (header 60px, composer pinned, scrollbar present, bottom-aligned content)
- System-chip grouping (`ui-event-grouping/verification.json`) — PASS
- Figure 4 visual regression (`ui-13-fixture/figure4-comparison.json`) —
  ok=true, 0 mismatched pixels (deterministic settle-poll capture)
- Scroll behaviour — `scripts/scroll_probe.sh` 5/5 PASS (t_6f308ca5)

## Remaining risks

- Pre-existing `cargo fmt` drift in `app.rs` (several hunks) predates this
  card and was left untouched; `presentation.rs` is fmt-clean.
- Headless captures cannot show a live peer network session; the normal
  conversation evidence uses the sanctioned deterministic fixture
  (`scripts/figure4_fixture.py`, t_ce8cc404), not production sample data.
