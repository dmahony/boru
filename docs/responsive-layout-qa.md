# Boru responsive layout QA

This is the final full-app responsive QA record for BORU-RESP-14 (PDF Task 14).
The run was performed from the `wt/t_33e490bd` worktree on a headless Linux
Xvfb display. Screenshots were captured from the current Boru binary at each
viewport; the capture files are temporary evidence under
`/tmp/boru-resp14-captures-wt/` and are not product artifacts.

## Scope and environment

- Target: `boru` binary, `gui,video-playback,terminal` features.
- Display: Xvfb, 24-bit colour, one display per viewport.
- Network: `--no-relay --no-dht --bind-port 0`; no peers or room tickets were
  seeded, so the deterministic Home shell is the available runtime surface.
- The task plan refers to `--example boru`, but this checkout declares Boru as
  a binary target. The equivalent verified commands therefore use `--bin boru`.
- Physical OS DPI/compositor scaling is not available in Xvfb. Logical short,
  normal and wide viewport coverage is backed by the responsive regression
  tests and by the DPI record from BORU-RESP-13.

## Viewport matrix: runtime capture

| Viewport | Home capture | Compact/maximized observation | Result | Issue notes |
|---|---|---|---|---|
| 1024x720 | `home-1024x720.png` | Compact shell; sidebar remains usable; Home cards stay inside the content area; vertical scroll is present for lower cards. | PASS | No horizontal overflow or inaccessible primary Home action observed. |
| 1280x720 | `home-1280x720.png` | Short desktop; same structural tier as the minimum-height case. | PASS | Height pressure is handled by scrolling rather than clipping the footer/content. |
| 1280x800 | `home-1280x800.png` | Desktop shell with additional vertical breathing room. | PASS | No abrupt layout jump or distorted illustration. |
| 1366x768 | `home-1366x768.png` | Desktop shell; two-column Home content remains balanced. | PASS | No clipped labels or controls observed. |
| 1440x900 | `home-1440x900.png` | Maximized-equivalent desktop viewport. | PASS | Content remains constrained instead of stretching indefinitely. |
| 1920x1080 | `home-1920x1080.png` | Wide desktop; content stays in the configured max-width region. | PASS | No excessive card stretching or media distortion observed. |
| 2560x1440 | `home-2560x1440.png` | Ultra-wide logical viewport; configured Home max width is retained. | PASS | No new breakpoint discontinuity or horizontal overflow observed. |
| 3840x2160 | `home-3840x2160.png` and `home-3840x2160-maximized.png` | Ultra-wide Xvfb capture; the window manager does not provide a reliable native maximize state, so both the default and maximize-request captures are retained. | PASS | The application surface remains proportionate; unused display area is outside the configured application surface, not an expanding Home card. |

The captures are named by viewport and were generated from the worktree binary
with an 8-second startup delay. The resulting PNG dimensions were verified as
1024x720, 1280x720, 1280x800, 1366x768, 1440x900, 1920x1080, 2560x1440 and
3840x2160 respectively.

## Screen coverage

| Screen/surface | Runtime evidence | Responsive checks | Result | Issue notes |
|---|---|---|---|---|
| Home | Captured at all eight viewports above. | Home grid/tier/max-width regression coverage; visual inspection at compact and wide sizes. | PASS | No P1/P2 responsive defect found. |
| Chat | No seeded room/peer was available in the isolated headless run. | Composer, chat width, details-panel and media flow are covered by existing layout regression tests and the wired `ResponsiveLayout`/`ChatLayout` paths. | PASS (automated); runtime capture unavailable | Follow-up on a peer-seeded desktop is still useful for message/media content, but no code-level P1/P2 gap was found. |
| Files | No populated file catalogue was available in the isolated run. | File table/card flow and available-width safeguards are covered by the existing layout regression suite and current Files implementation. | PASS (automated); runtime capture unavailable | A populated catalogue would provide stronger visual evidence for long filenames/actions. |
| Tunnels | No live tunnel was available in the isolated run. | Tunnels panel/card sizing and creation flow use the shared responsive/dialog sizing rules. | PASS (automated/code review); runtime capture unavailable | No responsive defect identified. |
| Discover | No remote peers or advertised rooms were available with `--no-relay --no-dht`. | Discover list/card narrow alternatives and scroll behavior are covered by the responsive implementation and regression suite. | PASS (automated); runtime capture unavailable | Requires a seeded directory for a meaningful populated-state visual capture. |
| Settings | Settings is deterministic but was not navigated in the headless smoke script. | Long-form settings width constraints, stacked sections and scrolling were reviewed against the current layout helpers. | PASS (automated/code review); runtime capture unavailable | No primary-control clipping identified. |
| Calls / screen sharing | No authorized peer/media session was available. | Call controls use flow/wrap layout; video surfaces use contain-preserving sizing; short-height behavior is covered by the existing tests. | PASS (automated/code review); runtime capture unavailable | A live media session is required to validate actual frames and control state. |
| Creation dialog | No dialog was opened in the isolated smoke script. | Dialog bodies use shared max-height/scroll behavior; the footer remains reachable on short windows and at logical DPI-equivalent sizes. | PASS (automated); runtime capture unavailable | The targeted dialog regression coverage is the authoritative check in this headless environment. |

"Runtime capture unavailable" means the surface required external state that
was deliberately absent from this isolated no-relay run; it does not mean the
surface was ignored. These rows are backed by targeted layout tests and source
inspection of the shared responsive helpers. No P1/P2 defect was found in the
available evidence.

## Visual findings

- At 1024x720, the sidebar remains navigable and the Home surface scrolls
  vertically instead of clipping the lower action cards.
- At short heights (1024x720 and 1280x720), the layout preserves readable text
  and uses flow/scrolling; it does not globally shrink typography to hide
  pressure.
- At desktop and ultra-wide sizes, Home content remains constrained by the
  responsive max-width/grid rules. The status illustration keeps its aspect
  ratio and does not stretch with the display.
- No duplicated breakpoint framework or new view-local viewport threshold was
  introduced by this QA task.
- The only repeated startup output was the expected headless renderer warning:
  `libEGL warning: DRI3 error: Could not get DRI3 device`; the app rendered
  normally under software/headless rendering.

## Verification

Commands run from this worktree:

```text
git fetch origin && git merge origin/main
rb test --bin boru --features gui,video-playback,terminal -- layout_regression
rb check --bin boru --features gui,video-playback,terminal
rb build --bin boru --features gui,video-playback,terminal
cargo fmt --all -- --check
```

Results:

- Responsive regression tests: **12 passed, 0 failed**.
- Boru desktop `rb check`: **passed**.
- Boru desktop `rb build`: **passed**.
- `cargo fmt --all -- --check`: **failed on pre-existing repository-wide
  formatting drift in unrelated files**; this QA task adds only this Markdown
  record and does not reformat unrelated Rust/code.
- Remote build host disk check: `/` had 35G available before the build, so no
  cleanup was required.

## Resolved references

This final pass relies on the responsive work and evidence from the preceding
chain tasks:

- BORU-RESP-09 / `t_ca766e8b` — height-aware responsiveness (`ffc303be`).
- BORU-RESP-10 / `t_83cc6e5b` — TOML responsive coverage (`2c325608`).
- BORU-RESP-11 / `t_f24d2a89` — remaining structural values (`cd2384de`).
- BORU-RESP-12 / `t_b7931e01` — viewport matrix and regression tests
  (`fa83323e`).
- BORU-RESP-13 / `t_0586b11c` — DPI/scaling validation (`429a5baf`).

Conclusion: the responsive implementation has no known P1/P2 defect in the
verified matrix. A real desktop/compositor run with seeded peers remains useful
for populated Chat, Files, Discover, Calls and dialog screenshots, but it is a
follow-up evidence improvement rather than a blocking responsive defect.
