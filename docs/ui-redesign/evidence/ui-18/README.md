# UI-18 evidence: Responsive resizing and high-DPI validation (t_f75e5521)

Evidence for the Phase 5 UI-18 card: the redesigned Boru home and chat screens
must stay professional across the four supported desktop window sizes and at
125/150/200 % scale factors. Every capture below was produced by the **real
running Boru GUI** under Xvfb from a deterministic QA fixture (never production
data), driven through the loopback MCP (`--mcp --enable-gui-test-actions`).

## TASK: t_f75e5521 — UI-18 Responsive resizing and high-DPI validation

## STATUS: Ready for Review

## SUMMARY

Captured and verified the full responsive matrix at 1024×720, 1280×800,
1440×900 and 1920×1080 for both the home (ChatList) and chat screens, plus a
long-value stress set (long friend labels, long group name, 2176-char unbroken
message, long URL, long system event), a 125/150/200 % high-DPI set at a
1280×800 logical window, and a continuous drag-resize sweep across nine window
sizes with a return-to-reference stability check. No horizontal scroll,
clipped labels, overlapping controls, or unreadably compressed cards were
found at any required size; no blurry icons or incorrectly scaled text at any
DPI factor; the app remains fully usable at 1024×720 with body font size
unchanged. Breakpoints documented below.

## Changed files

- `examples/iced_chat/app.rs` — **real production fix from this card**: sidebar
  conversation and group rows with long names wrapped *inside* their clip
  containers, growing each row to 4–5 lines instead of truncating (visible in
  the pre-fix stress capture). Added `Wrapping::None` to the conversation
  name, the last-message preview, and the group name so every long-value row
  stays single-line and ellipsizes at the clip boundary (tooltips still show
  the full value). Verified by the post-fix stress captures below.
- `scripts/ui18_fixture.py` (new) — deterministic long-value stress fixture
  (extends `figure4_fixture` with long labels/names/messages/events).
- `scripts/ui18_responsive_evidence.sh` (new) — Xvfb + MCP capture harness
  for the matrix, stress, DPI and resize-sweep evidence.
- `docs/ui-redesign/evidence/ui-18/` (new) — this evidence set.

The remaining responsive rules implemented by UI-04..UI-16 (design-token
sidebar width clamp, quick-action column breakpoints, home rail stacking, chat
bubble width cap, fixed header/composer with the timeline as the only expanding
region) already satisfy every other UI-18 acceptance criterion — no further
production changes were required.

## Evidence artifacts

| File | Shows |
|---|---|
| `ui18_home_{1024x720,1280x800,1440x900,1920x1080}.png` | Home/ChatList screen at each required viewport |
| `ui18_chat_{1024x720,1280x800,1440x900,1920x1080}.png` | Chat conversation (Figure 4 fixture) at each required viewport |
| `ui18_stress_home_{1024x720,1280x800}.png` | Home with long friend labels, long group name, populated rail |
| `ui18_stress_chat_{1024x720,1280x800}.png` | Chat with long unbroken message, long URL, long system event |
| `ui18_home_dpi{1.25,1.5,2.0}.png` | Home at 125/150/200 % scale (1280×800 logical) |
| `ui18_chat_dpi{1.25,1.5,2.0}.png` | Chat at 125/150/200 % scale (1280×800 logical) |
| `ui18_sweep_{1..9}_*.png` | Continuous drag-resize sweep frames (see table below) |

### Resize sweep frames (one live instance, home screen)

| Frame | Size | Purpose |
|---|---|---|
| 1 | 1024×720 | smallest supported |
| 2 | 1080×720 | below the 2-column quick-action boundary |
| 3 | 1152×768 | intermediate |
| 4 | 1280×800 | reference (start) |
| 5 | 1366×768 | laptop width |
| 6 | 1440×900 | large |
| 7 | 1600×900 | intermediate |
| 8 | 1920×1080 | full HD |
| 9 | 1280×800 | reference again — stability re-check |

Frame 9 vs frame 4 (same window size after a full sweep) shows the layout is
stable: only timestamp text differs (pixel diff ≈ 1 k px of 1 M, all in the
relative-time labels), no column-count oscillation, no jump. MCP stayed
responsive after the sweep (app did not crash or freeze during resizing).

## How it was run

- Build: `cargo build --features gui --example boru` (isolated worktree at
  HEAD `daa44f2b`; the shared kanban tree did not compile because sibling FS
  workers were mid-edit — documented in the worker report).
- Fixtures: `figure4_fixture.py inject` (Figure 4 timeline + friends +
  conversations) and `ui18_fixture.py stress` (long-value overlay).
- Launch: `boru --data-dir <tmp> --no-dht --no-relay --mcp
  --enable-gui-test-actions --mcp-bind 127.0.0.1:<port>`; home captures omit
  the `open` subcommand so the lobby subscription returns the UI to the chat
  list (`return_to_chat_list_after_open`), exactly as UI-11/UI-15 captured
  home; chat captures pass `open` and then `boru_gui_open_conversation`.
- Presence: `boru_gui_set_peer_presence {online:true}` routes through the
  production friend-status path so the Online Peers rail and Recent Activity
  show truthful live data.
- Capture: Xvfb at the target size, `xdotool windowsize` the window,
  `settle` (wait for two byte-identical frames), ImageMagick `import -window`.
- DPI: `WINIT_X11_SCALE_FACTOR=1.25|1.5|2.0` with the Xvfb screen and window
  sized to the physical resolution (logical 1280×800 × factor); winit derives
  the logical size from the env override (winit X11 documented behavior).
- Visual QA: screenshots inspected with a vision model and pixel analysis
  (ImageMagick histogram/compare); no clipping/overlap/horizontal-scroll
  found at any size or factor.

## Final breakpoints (worker handoff)

Implemented by the design system and verified at every required size:

| Breakpoint | Rule |
|---|---|
| `< 640 px` window width | Quick actions collapse to 1 column; only reached below the supported minimum |
| `640–1039 px` | Quick actions: 2 columns |
| `≥ 1040 px` | Quick actions: 4 columns |
| `< 900 px` | Home rail (Online Peers / Recent Activity / Tunnels) stacks below the hero + actions instead of sitting 1/3 right |
| `≥ 1024 px` | Two-thirds content + one-third rail (plan §4 reference layout) |
| 1024–1280 | Sidebar width interpolates 288→304 px (`sidebar_width_for`) |
| `≥ 1440 px` | Main content padding 32 px (`is_large`) |
| 1024–1439 | Main content padding 24 px |
| `≤ 1024 px` | Main content padding 16 px (`is_compact`) |
| any | Chat bubble max width = min(560 px, 68 % of timeline) (`chat_bubble_max_width`) |
| any | Composer pinned below the timeline; the timeline is the only expanding region |
| any | Long names in the chat header and group rows use `Wrapping::None` + `.clip(true)` with tooltips; peer keys truncate to `8…4` with a copy button and full-key tooltip |

### Intentionally hidden / collapsed secondary content

- The home rail is *not* hidden at 1024×720; it stacks vertically below the
  hero so every card keeps a readable width (verified: no compressed cards).
- Sidebar sections remain individually collapsible (`sidebar_section_collapsed`)
  — no section is force-hidden by size.
- At 1024×720 the quick-action grid deliberately uses 2 columns (not 4) so
  cards keep their description text; this is the plan §4 medium arrangement,
  not a clipping workaround.

## Acceptance criteria

- [x] No horizontal scroll, clipped labels, overlapping controls or
      unreadably compressed cards at 1024×720 / 1280×800 / 1440×900 /
      1920×1080 — verified on home and chat in the matrix and stress captures.
- [x] No blurry icons or incorrectly scaled text at 125/150/200 % — verified
      in the DPI captures (tiny-skia renders at physical resolution, so
      higher scale is crisper, never blurry).
- [x] App remains usable at 1024×720 without reducing body font size — body
      text sizes come from design tokens and are unchanged by window size;
      layout reflows instead (rail stacks, quick actions 2-up).

## Verification required before Done

- [x] Complete screenshot matrix attached (this directory).
- [x] Continuous drag-resize performed on home (sweep frames 1–9) and chat
      (matrix at all four sizes + live-resize path used by UI-16 evidence).
- [x] Platform-specific rendering checks: full `cargo test --features gui
      --example boru` suite (see worker report) — the GUI example is not
      exercised in CI (Linux X11/Wayland runtime), so the local Xvfb captures
      are the platform rendering evidence.

## Remaining risks

1. **Shared-tree build** — the kanban shared tree did not compile during this
   run because sibling FS workers were mid-edit (missing `message_trace_label`
   arms for new `AppMessage` variants). All evidence was produced from an
   isolated worktree at HEAD (which includes UI-17), and the UI-18 harness
   scripts are self-contained; no UI-18 regression is implied.
2. **Lobby race** — with the `open` subcommand the app deterministically lands
   on the lobby chat once the lobby subscription completes; home captures must
   launch without `open` (documented in the harness). This is production
   behavior (`return_to_chat_list_after_open`), not a UI defect.
3. **DPI env var** — `WINIT_X11_SCALE_FACTOR` is winit's documented X11
   override; Windows/macOS scaling uses the OS-reported factor and was not
   re-tested here (no Windows/macOS hosts available).

## Plan section 6 worker report

See `../ui-18-worker-report.md` (or the card comment) for the full template:
commands run, results, acceptance mapping and follow-up.
