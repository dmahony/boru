# UI-16 evidence: Chat footer, full composition and scroll polish (t_dfbde136)

Evidence for the Phase 4 UI-16 card: restrained route/peer status footer,
full chat-screen composition and edge-case presentation, verified against
Figure 4 of the Boru Modern UI implementation plan.

## TASK: t_dfbde136 — UI-16 Chat footer, full composition and scroll polish

## STATUS: Ready for Review

## SUMMARY

Added a restrained footer/status line below the chat composer (plan UI-16
steps 128-130): route derived from real connection state (Direct (mesh) /
Relay / Mesh / Not connected) with a muted mesh icon on the left, and the
connected peer count on the right. The header already owns presence +
encryption (direct chats) and member count (group chats), so the footer
shows only complementary route/peer state — no duplicated status text.
Footer content aligns with the composer inner edges (SPACE_4 inside the
panel's SPACE_16), the timeline remains the only vertically expanding
region, and panel bottom padding was tightened to SPACE_12 with an 8 px gap
between composer and footer.

Full evidence set re-captured at all four required viewports after fixing a
fixture bug: `figure4_fixture.py` wrote the spec's lowercase delivery state
(`"seen"`), which the app's case-sensitive `DeliveryState` enum rejects,
causing the whole `chat_history.json` to fail parsing and the timeline to
render empty. The fixture now normalises delivery-state spellings to the
app's serde variant names, and all Figure-4 bubbles replay correctly.

663/663 tests pass; cargo check clean.

## CHANGED FILES

- `examples/iced_chat/app.rs`: `chat_footer_status()` pure helper + footer
  wiring in the chat column (committed as 0eca0d72).
- `examples/iced_chat/ui_components.rs`: `chat_status_footer()` component
  (committed as 0eca0d72).
- `scripts/figure4_fixture.py`: normalise lowercase delivery-state spellings
  from the Figure 4 spec to the app's `DeliveryState` enum variant names so
  injected history parses and replays.
- `scripts/ui16_fixture.py` (new): empty / one-message / long-history data
  builders on top of `figure4_fixture`.
- `scripts/ui16_evidence.sh` (new): Xvfb capture harness for all scenarios.
- `scripts/ui16_verify.py` (new): OCR verification of the captures.
- `scripts/ui16_side_by_side.py` (new): target-vs-implementation montage.
- `docs/ui-redesign/evidence/ui-16/` (new): this evidence set:
  - `t_dfbde136_figure4_{1024x720,1280x800,1440x900,1920x1080}.png`
  - `t_dfbde136_empty_{1024x720,1280x800}.png`
  - `t_dfbde136_one_1280x800.png`
  - `t_dfbde136_long_1280x800.png`
  - `t_dfbde136_offline_1280x800.png`
  - `t_dfbde136_live_resize_{1024x720,1280x800,1440x900,1920x1080}.png`
  - `t_dfbde136_side_by_side_1280x800.png`
  - `verification.json` — machine-readable checks.

## DESIGN AND ARCHITECTURE DECISIONS

- Complementary status split (step 129): header = presence + encryption
  (direct) or member count (group); footer = connection route + peer count.
- Footer uses existing primitives only: `Icon::Mesh` at `IconSize::Xs`,
  `fonts::XS` text, `design_tokens::text_secondary` (connected) /
  `text_muted` (disconnected), and existing spacing tokens.
- Timeline is the sole expanding region, so the latest message stays visible
  above the composer on resize and new-message insertion (step 132).
- QA-only fixture data lives in isolated temp data dirs; no production
  sample data and no changes to persistence keys, commands, network events
  or public APIs.

## BEHAVIOR PRESERVATION

- Header actions, toolbar, message grouping, delivery states and the UI-15
  composer are unchanged; footer is purely additive.
- Route/peer state derives from the same `neighbors`/`peer_presence` state
  used by the existing header tooltip and details panel (truthful states).
- Existing tests all pass (663/663), including the 8 new footer tests.

## COMMANDS RUN

- Build: `cargo build --features gui --example boru` (binary at
  target/debug/examples/boru, built 02:51).
- Tests: `cargo test --features gui --example boru` → 663 passed, 0 failed.
- Formatting/lint: `cargo fmt --check` (see note below).
- Screenshot command: `bash scripts/ui16_evidence.sh`.
- Verification: `python3 scripts/ui16_verify.py docs/ui-redesign/evidence/ui-16`
  → 13/13 PASS.
- Side-by-side: `python3 scripts/ui16_side_by_side.py`.

## RESULTS

- Build result: PASS.
- Test result: 663 passed, 0 failed, 0 ignored.
- OCR verification: 13/13 captures PASS (footer route label present, no E2EE
  duplication in the footer band, peer count / offline state correct,
  long-history and live-resize messages visible above the composer).
- Visual review: header, bubbles, composer and footer match Figure 4's
  vertical composition at 1280 x 800; no message hidden behind composer or
  footer.

## VISUAL EVIDENCE

- `t_dfbde136_figure4_1280x800.png` — reference-size full chat screen.
- `t_dfbde136_side_by_side_1280x800.png` — Figure 4 target beside
  implementation.
- `t_dfbde136_empty_1280x800.png` / `t_dfbde136_empty_1024x720.png` —
  empty conversation.
- `t_dfbde136_one_1280x800.png` — one-message conversation.
- `t_dfbde136_long_1280x800.png` — 400-message history pinned to latest.
- `t_dfbde136_offline_1280x800.png` — footer "Not connected" state.
- `t_dfbde136_live_resize_{...}.png` — live resize while receiving messages
  at all four viewports.

## ACCEPTANCE CRITERIA

- [x] Complete chat screen closely matches Figure 4 at 1280 x 800 —
      side-by-side + figure4 captures.
- [x] No message hidden behind the composer/footer — long history and
      live-resize captures show the latest message above the composer.
- [x] Header and footer show truthful, non-duplicated status — header owns
      presence/encryption, footer owns route/peer count; OCR confirms no
      E2EE duplication in the footer band.
- [x] Empty and long-history layouts remain polished — dedicated captures.

## KNOWN LIMITATIONS OR RISKS

- `cargo fmt --check` may report pre-existing unstaged formatting diffs in
  `app.rs` test assertions from other in-flight tasks in this shared tree;
  no new formatting issues introduced by this task's hunks.
- The live-resize capture at 1280x800 and 1920x1080 OCR the peer count in
  the full-page pass but not in the bottom-band crop; visual inspection
  confirms "Relay · 1 peer" is present.

## SUGGESTED FOLLOW-UP

- UI-17 (t_8de1c9c4) — real-state integration and behavior regression pass;
  footer route/peer state should be exercised with real peers.
