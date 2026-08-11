# BORU-HOME-12 — Regression + visual QA gate (final)

Task: `t_a38b6ffa` — final gate of the Boru_Home_Screen_Refinement batch.

## Build

Release build on debsrv via `rb`, from the canonical repo state (post
BORU-HOME-01..11, HEAD `fdcf5708`):

```text
rb build --release --example boru --features gui,video-playback,terminal
Finished `release` profile [optimized] target(s) in 13m 37s
exit code: 0
```

Final compile gate (also on debsrv):

```text
rb check --example boru --features gui,video-playback,terminal
Finished `dev` profile [unoptimized + debuginfo] target(s) in 17.47s
exit code: 0   (only the pre-existing unfulfilled #[expect(dead_code)] warnings)
```

The release binary was rsynced back to `target/release/examples/boru`
(53,427,128 bytes, mtime after HEAD) and every capture below ran against it
under Xvfb with a fresh temporary data dir, `--no-dht --no-relay`, MCP +
GUI test actions, and the loopback MCP test interface.

## Visual QA — final screen at every supported width

| Capture | Surface | Purpose |
|---|---|---|
| `t_a38b6ffa_home_1600x900.png` | 1600 × 900 | normal desktop, two-column dashboard |
| `t_a38b6ffa_home_1920x1080.png` | 1920 × 1080 | maximized (BORU-HOME-11 Shrink-height check) |
| `t_a38b6ffa_home_1280x800.png` | 1280 × 800 | medium, two-column |
| `t_a38b6ffa_home_1024x720.png` | 1024 × 720 | narrow, single-column transition |
| `t_a38b6ffa_home_800x600.png` | 800 × 600 | minimum supported width |
| `*_scrolled.png` | scrolled states | proves below-fold content exists (quick actions, tunnels) |
| `geometry.txt` | OCR word-box geometry | **words_past_right_edge = 0 at every width** — no horizontal clipping |

Visual inspection of every capture: the final screen keeps the intended
hierarchy — greeting → connection hero → Mesh Health → quick actions →
Download Manager → People & Activity → Tunnels — with consistent card
radii/padding (RADIUS_CARD), no alignment errors, no clipped text, and no
large unused blank area at maximized height (the BORU-HOME-11 Shrink fix).
The 1920x1080 maximized capture is compact top-aligned, not stretched.

## Baseline comparison (BORU-HOME-01, t_a0b1f82f)

`scripts/compare_screenshot.py` produced red-on-black diff images against the
baseline (the redesign intentionally changes pixels; the diff localizes the
change):

| Size | mismatch fraction | diff |
|---|---|---|
| 1280 × 800 | 0.1137 | `diff_vs_baseline_1280x800.png` |
| 1920 × 1080 | 0.0664 | `diff_vs_baseline_1920x1080.png` |

Diffs are concentrated in the dashboard content (hero, quick-action grid,
merged People & Activity card, Tunnels empty state, spacing/colour tokens);
the sidebar and overall structure are preserved. No feature regions were
removed — the merged Online Peers + Recent Activity (BORU-HOME-05) is the
only structural consolidation, per the design plan.

## Interactive regression QA (release binary, real pointer clicks)

`scripts/ui_home12_qa.sh` drives the production UI paths (mouse click →
AppMessage → update → view) and verifies the result by MCP GUI snapshot +
OCR (`qa_ocr.txt`). All passed:

- Home dashboard zero state: greeting, connection hero, Mesh Health,
  quick actions, Download Manager, People & Activity, Tunnels empty state.
- **All four quick actions** (BORU-HOME-07 redesign):
  - Start Chat → Friend Requests screen (snapshot `FriendRequests`)
  - Create Group → "Create Group Chat" dialog
  - Create Public Room → "Create Public Room" dialog
  - Create Tunnel → "Create Tunnel" dialog
- Download Manager access → "Files I'm Sharing" screen.
- Live updates: peer presence flip 0 → 1 → 2 → 0 shows Ada/Bob rows and
  "came online" / "went offline" activity entries.
- Sidebar navigation destinations (Friends, Settings) reachable.
- Long-identifier state: a 64-char friend key + 60-char label truncates in
  the sidebar — `words_past_right_edge = 0` at 1280 px; the layout does not
  resize to fit long identifiers.
- App alive at end of run; no panic in the app log.

## Harness notes

- The `open` subcommand auto-opens a chat room whose OpenRoom task races
  `boru_gui_navigate chat_list`; the harness waits for the dashboard to
  settle (2 consecutive `ChatList` snapshots) before capturing, and
  re-navigates before every quick-action click so coordinates are always
  calibrated against the dashboard.
- Quick-action click targets are calibrated from live captures
  (`scripts/ui17_click_calibrate.py`) with a y-band that excludes the rail's
  "Create tunnel" action, which otherwise breaks the line-aware phrase
  match; each card is anchored on a unique single word (Start / Group /
  Public / Tunnel).

## Conclusion

All acceptance criteria met: no home-screen feature lost or broken, clear
hierarchy, balanced at normal + maximized sizes, no alignment/clipping/
blank-area issues, no networking or data-layer change required. No code
changes were needed — the batch is ready to ship as merged.
