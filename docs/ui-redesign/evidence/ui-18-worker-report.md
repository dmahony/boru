# UI-18 worker report — Responsive resizing and high-DPI validation (t_f75e5521)

## TASK: t_f75e5521 — UI-18 Responsive resizing and high-DPI validation
## STATUS: Ready for Review

## SUMMARY

Captured and verified the full responsive matrix at 1024×720, 1280×800,
1440×900 and 1920×1080 for both the home (ChatList) and chat screens, plus a
long-value stress set (long friend labels, long group name, 2176-char unbroken
message, long URL, long system event), a 125/150/200 % high-DPI set at a
1280×800 logical window, and a continuous drag-resize sweep across nine window
sizes with a return-to-reference stability check.

**Real production bug found and fixed by this card:** sidebar conversation and
group rows with long names wrapped *inside* their clip containers, growing
each row to 4–5 lines instead of truncating (visible in the pre-fix stress
capture). Fix: `Wrapping::None` on the conversation name, the last-message
preview, and the group name (examples/iced_chat/app.rs). Post-fix stress
captures show every long-value row single-line and ellipsized; tooltips still
expose the full value.

No horizontal scroll, clipped labels, overlapping controls, or unreadably
compressed cards at any required size; no blurry icons or incorrectly scaled
text at any DPI factor (native 2x supersampling verified: 2x render shows
7.3 % RMSE vs a plain upscale in a static region, and is visibly sharper than
1x when downscaled); the app remains fully usable at 1024×720 with body font
size unchanged (layout reflows instead — quick actions 2-up, home rail
stacked).

## CHANGED FILES

- `examples/iced_chat/app.rs`: production fix — `Wrapping::None` on (1) the
  sidebar conversation name, (2) the last-message preview, (3) the group name
  in the sidebar groups section. Prevents long values from wrapping rows
  taller inside their clip containers; rows now truncate single-line with
  tooltips for the full value.
- `scripts/ui18_fixture.py` (new): deterministic long-value stress fixture
  (extends `figure4_fixture` with long labels/names/messages/events).
- `scripts/ui18_responsive_evidence.sh` (new): Xvfb + MCP capture harness for
  the matrix, stress, DPI and resize-sweep evidence.
- `docs/ui-redesign/evidence/ui-18/` (new): evidence set (README.md,
  verification.json, 26 screenshots).
- `docs/ui-redesign/evidence/ui-18-worker-report.md` (new): this report.

## DESIGN AND ARCHITECTURE DECISIONS

- No layout-architecture changes were needed: the responsive rules from
  UI-04..UI-16 (design-token sidebar width clamp, quick-action column
  breakpoints, home rail stacking, chat bubble width cap, fixed
  header/composer with the timeline as the only expanding region) already
  satisfy every UI-18 acceptance criterion once the long-value wrapping defect
  is fixed.
- The truncation fix follows the existing design pattern already used by other
  rows in the codebase (`Wrapping::None` + `.clip(true)` + tooltip on
  overflow) — no new primitives, tokens or public APIs.
- DPI is handled by the framework: winit X11 derives the scale factor from
  `WINIT_X11_SCALE_FACTOR` (or Xft.dpi), and the tiny-skia renderer draws at
  physical resolution × scale — native supersampling, never bitmap upscaling.

## BEHAVIOR PRESERVATION

- All original actions/state paths are unchanged; the fix only alters text
  wrapping for long values (previously they wrapped; now they truncate with a
  tooltip). No commands, message states, persistence keys, network events or
  public APIs were removed or renamed.
- Verified live via MCP (`boru_gui_open_conversation`,
  `boru_gui_set_peer_presence`, `boru_gui_navigate`) on the real running app —
  presence, activity rail, peer counts and footer all show truthful state.
- 664/664 example unit tests pass on the isolated worktree (same count as the
  UI-17 baseline).

## COMMANDS RUN

- Build: `cargo build --features gui --bin boru` — exit 0 (isolated
  worktree at HEAD `daa44f2b` + the app.rs fix; the kanban shared tree did not
  compile because sibling FS workers were mid-edit — documented in Remaining
  risks).
- Tests: `cargo test --features gui --bin boru` — 664 passed, 0 failed
  (log: ui18-tests.log, summarized in this evidence set).
- Formatting/lint: `cargo fmt --check` not run on the shared tree (siblings
  mid-edit); the 3 hunks are rustfmt-clean and follow the existing file style.
- Screenshot: `scripts/ui18_responsive_evidence.sh` (matrix, stress, DPI,
  sweep) + `scripts/ui18_stress_recapture.sh` (post-fix stress re-capture).

## RESULTS

- Build result: PASS (worktree HEAD + fix).
- Test result: 664/664 PASS.
- Warnings or baseline failures: none new; the shared tree remains
  uncompilable only because of sibling FS workers' in-flight edits (missing
  `dashboard_view_model` symbols referenced from app.rs), which reproduce
  without UI-18 hunks.

## VISUAL EVIDENCE

`docs/ui-redesign/evidence/ui-18/`:

| File | Shows |
|---|---|
| `ui18_home_{1024x720,1280x800,1440x900,1920x1080}.png` | Home at each required viewport |
| `ui18_chat_{1024x720,1280x800,1440x900,1920x1080}.png` | Chat (Figure 4 fixture) at each required viewport |
| `ui18_stress_home_{1024x720,1280x800}.png` | Long names/labels, group name, populated rail (post-fix) |
| `ui18_stress_chat_{1024x720,1280x800}.png` | Long unbroken message, long URL, long system event (post-fix) |
| `ui18_home_dpi{1.25,1.5,2.0}.png` | Home at 125/150/200 % scale (1280×800 logical) |
| `ui18_chat_dpi{1.25,1.5,2.0}.png` | Chat at 125/150/200 % scale (1280×800 logical) |
| `ui18_sweep_{1..9}_*.png` | Continuous drag-resize sweep frames |

Pre-fix vs post-fix: the original stress capture showed the long-name rows at
4–5 lines; the re-captured post-fix set shows single-line ellipsized rows.
See `README.md` and `verification.json` in the evidence dir for the full
breakpoint table, sweep frame table, and machine-readable checks.

## ACCEPTANCE CRITERIA

- [x] No horizontal scroll, clipped labels, overlapping controls or
      unreadably compressed cards at 1024×720 / 1280×800 / 1440×900 /
      1920×1080 — verified on home and chat in the matrix and stress captures.
- [x] No blurry icons or incorrectly scaled text at 125/150/200 % — verified
      in the DPI captures (native supersampling; 2x render is sharper than 1x
      when downscaled, not an upscale).
- [x] App remains usable at 1024×720 without reducing body font size — body
      text sizes come from design tokens and are unchanged; the layout reflows
      (quick actions 2-up, home rail stacked below the hero).

## VERIFICATION REQUIRED BEFORE DONE

- [x] Complete screenshot matrix attached (26 PNGs in the evidence dir, all
      at exact required dimensions).
- [x] Continuous drag-resize performed on home (sweep frames 1–9 across
      `1024x720 → 1080x720 → 1152x768 → 1280x800 → 1366x768 → 1440x900 →
      1600x900 → 1920x1080 → 1280x800`); frame 9 vs frame 4 pixel diff = 231
      of ~1M px (timestamps only) — no column-count oscillation, no jitter,
      MCP responsive after the sweep. Chat resize exercised by the same live
      `xdotool windowsize` path at all four viewports in the matrix.
- [x] Platform-specific rendering checks: the GUI example is not exercised in
      CI (Linux X11/Wayland runtime), so the Xvfb captures + 664-test suite
      are the platform rendering evidence; winit scale-factor behavior is
      documented (WINIT_X11_SCALE_FACTOR / Xft.dpi).

## KNOWN LIMITATIONS OR RISKS

1. **Shared-tree build** — the kanban shared tree did not compile during this
   run because sibling FS workers were mid-edit (missing
   `dashboard_view_model` symbols referenced from app.rs). All evidence was
   produced from an isolated worktree at HEAD (includes UI-17) + the UI-18
   fix, and the fix hunks apply cleanly to the shared tree. No UI-18
   regression is implied.
2. **DPI on other platforms** — `WINIT_X11_SCALE_FACTOR` is winit's documented
   X11 override; Windows/macOS use the OS-reported scale factor and were not
   re-tested here (no Windows/macOS hosts available). The same iced/tiny-skia
   rendering path applies.
3. **Lobby race** — with the `open` subcommand the app deterministically
   lands on the lobby chat; home captures launch without `open`
   (`return_to_chat_list_after_open`), exactly as UI-11/UI-15 did. This is
   production behavior, not a UI defect.

## SUGGESTED FOLLOW-UP

- UI-19 (keyboard/focus) can build directly on this card's layout; the
  truncation/tooltip pattern is now consistent across the sidebar.
- When the FS sibling burst settles, run one clean-checkout build + full
  suite on the shared tree to close the concurrency risk.
