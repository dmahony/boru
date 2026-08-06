# EPIC-UI-HOME-FONTS — Final Implementation Report

**Epic:** Boru Home Screen Refinement and Typography
**Epic card:** t_fdf744bb (parent: t_61a56729 — UI-HOME-19 release gate)
**Report date:** 2026-08-06
**Repo / branch:** iroh-gossip-chat @ `origin/main` (close-out commits 1a0bc8b3 report, 85f040a7 restore-revert; parent chain 7b012e81 → 7bc0b5c5)
**Report author:** orchestrator profile (epic close-out task t_fdf744bb)

---

## STATUS

**Complete with approved differences.**

All 19 UI-HOME-* cards (UI-HOME-01…UI-HOME-19) are Done with committed
evidence; all 21 UI-HOME commits (including the UI-HOME-followup for
t_d849e063) are present on `origin/main`; the working tree is clean; the
epic definition of done is satisfied with ten explicitly approved
differences (see REMAINING DIFFERENCES OR FOLLOW-UPS). This close-out made
no source changes — it only verified the chain and produced this report.

---

## SUMMARY

- **What changed:** The Boru home screen (`Screen::ChatList`) was rebuilt to
  match the approved Figure 3 dashboard mockup: a centred, max-width page
  container with a refined header (greeting + subtitle left, compact status
  pill right), a two-column dashboard grid (~66.7% / ~33.3% with 24 px column
  gap and 20 px vertical card gaps), a full connection/hero card with live
  state, a restored Mesh Health card with live summary and recent events,
  modernised quick-action cards, Online Peers / Recent Activity / Tunnels
  rail cards, intentional empty states, and a content-width-driven responsive
  system (wide / medium / narrow / minimum). A central semantic typography
  system (`fonts::TypeRole`, 15 roles) registers five font families at exact
  static weights; chat messages, sender labels, metadata and the composer
  moved to Figtree; major headings use Manrope; Source Sans 3 is the general
  UI font; Raleway ExtraBold is limited to the BORU wordmark; JetBrains Mono
  is limited to technical identifiers. All font roles are centralised as
  semantic tokens in `fonts.rs` + `design_tokens.rs` and documented in
  `DESIGN_SYSTEM.md`.

- **What intentionally did not change:** All networking, discovery, chat,
  room, group, file-sharing and tunnel business logic, payloads and state
  semantics (verified: `git diff` of `src/` across the whole epic is empty
  except one additive gated test command; `examples/iced_chat/mcp_server.rs`
  gained one gated test-harness action; `Cargo.lock` only syncs the boru-core
  0.116.1 → 0.117.0 version). The legacy `Typography` enum and legacy bundled
  fonts (Inter, variable Manrope, JetBrains Mono variable/italic) remain in
  place for compatibility but are not loaded by default. Dark theme remains
  out of scope.

- **Clipping root cause:** Text clipped on the home screen for three
  combined reasons: (1) fixed-height rows — Online Peers rows were fixed at
  60 px, Recent Activity rows at 32 px, Tunnels rows at 48 px — so any
  wrapped content (long display names, endpoints, descriptions) was cut off;
  (2) hidden-overflow masks — Recent Activity and Mesh-event descriptions
  were truncated to 40 chars, forced to `Wrapping::None` and painted inside
  `container.clip(true)`, hiding the overflow instead of allowing wrap; and
  (3) unbroken technical strings (64-char peer keys, tunnel endpoints, long
  display names in the greeting / hero / mesh status / sidebar identity)
  used `Wrapping::None` or `Shrink` widths and painted outside their rows.
  Quick-action cards additionally had fixed-height boxes in some states that
  could clip their descriptions. Fix (UI-HOME-06 + UI-HOME-10): rows became
  content-driven with zero-width min-height spacers preserving the approved
  rhythm; truncation and `.clip(true)` masks were removed; all long-string
  call sites switched to `width(Fill)` + `Wrapping::WordOrGlyph`; quick-action
  cards became fully content-driven. Verified: `words_past_right_edge = 0`
  and every approved quick-action description fully visible at all four
  widths (`quick_action_clip_check.txt` RESULT: PASS).

- **Main responsive rules:** Every responsive decision is driven by the
  available **content width** — window width minus the sidebar, the 1 px
  divider and both horizontal page paddings — computed by
  `home_content_width(window_width)` in `design_tokens.rs`. Six intentional
  breakpoints: `HOME_QUICK_FOUR_COL_CONTENT = 1000 px` (≥ → four quick-action
  columns), `HOME_TWO_COL_CONTENT = 720 px` (≥ → two dashboard columns),
  `HOME_ILLUSTRATION_FULL_CONTENT = 720 px` (≥ → full hero illustration),
  `HOME_COMPACT_HEADER_CONTENT = 560 px` (below → two-line card headers +
  pill under greeting), `HOME_QUICK_ONE_COL_CONTENT = 520 px` (below → one
  quick action per row), `HOME_ILLUSTRATION_HIDE_CONTENT = 520 px` (below →
  hero illustration hidden). At the four evidence windows the content widths
  are ~1231 / ~919 / ~679 / ~455 px, giving the verified tiers: 1600×900
  wide (two columns, 4-across quick actions, full hero), 1280×800 medium
  (two columns, 2×2 quick actions), 1024×720 narrow (single column, rail
  below, 2×2 quick actions, 0.8× hero), 800×600 minimum (single column,
  1-across quick actions, compact two-line headers, pill stacked, hero
  hidden). The page scrolls vertically via `gutter_scrollable`; no
  horizontal scrollbar is produced.

- **Typography architecture:** `fonts.rs` is the single source of truth.
  `load_fonts()` (fonts.rs:597) registers 12 static font faces across the
  five families — Source Sans 3 (400/500/600/700), Manrope (600/700),
  Figtree (400/500/600), Raleway ExtraBold (800), JetBrains Mono (400/500) —
  and is chained at startup (`main.rs:1602`). `TypeRole` (fonts.rs:357) is
  the canonical semantic-role enum with 15 roles (display_heading, page_title,
  section_title, card_title, body, body_emphasised, button_label,
  supporting_text, metadata, chat_message, chat_sender, chat_metadata,
  composer_text, technical_value, brand_wordmark); each role exposes
  `family_name()`, `weight()`, `size_px()`, `font()`, and explicit fallbacks
  (every role degrades to Source Sans 3 at the nearest registered weight;
  `technical_value` degrades to platform monospace). All required weights are
  real registered static instances — no synthetic bolding and no variable-font
  axis interpolation. `TypeRole` is used 458× in app.rs plus pervasively in
  ui_components.rs, shared_by_me_table.rs and other view modules. A regression
  test (`every_required_family_weight_is_registered`) pins the 11 required
  family/weight pairs. Screens were migrated to the roles in UI-HOME-12
  (home), UI-HOME-13 (chat/composer) and UI-HOME-14 (shared app chrome);
  the legacy `Typography` enum is retained for the few remaining call sites.

---

## FILES AND COMPONENTS

263 files changed across the 21 UI-HOME commits (bc5a949e…7bc0b5c5),
~90 % of them in `examples/iced_chat/` and `docs/ui-redesign/`.

- **Added:**
  - `examples/iced_chat/fonts/` — 9 new static font faces:
    `SourceSans3-Medium.ttf` (500), `Figtree-Regular.ttf` / `Figtree-Medium.ttf`
    / `Figtree-SemiBold.ttf` (400/500/600), `Manrope-SemiBold.ttf` /
    `Manrope-Bold.ttf` (600/700, instancer-generated from the official
    variable font), `JetBrainsMono-Regular.ttf` / `JetBrainsMono-Medium.ttf`
    (400/500, instancer-generated); plus `Figtree-OFL.txt`, `Manrope-OFL.txt`,
    `Raleway-OFL.txt`, `JetBrainsMono-OFL.txt` and `THIRD_PARTY_NOTICES.md`.
  - `examples/iced_chat/fonts.rs` — `TypeRole` enum, `figtree()` constructor,
    `FIGTREE` constant, expanded `load_fonts()`, family/weight constants,
    font-registration regression tests.
  - `examples/iced_chat/design_tokens.rs` — content-width breakpoint
    constants, `home_content_width()`, spacing/radius/shadow/token additions
    with tests.
  - `docs/ui-redesign/UI-HOME-01-audit.md` … `UI-HOME-19-gate.md`,
    `UI-HOME-followup-button-focus.md`, `DESIGN_SYSTEM.md` (rewritten),
    `docs/ui-redesign/evidence/` (per-card evidence dirs `t_<card-id>/`,
    ~19 dirs of PNG/text evidence), and 20+ evidence-harness scripts under
    `scripts/ui_home*.sh` / `ui_home*.py`.
  - `src/diagnostics.rs` — additive `GuiTestCommand::ClearMeshEventLog`
    variant (gated test command; only source-file change outside
    examples/iced_chat/docs).

- **Modified:**
  - `examples/iced_chat/app.rs` — home screen view/update layers
    (`view_chat_list_content`, hero card, quick-action grid, rail cards,
    compact headers, pill stacking, live-state wiring, keyboard/focus
    handling), TypeRole adoption, scroll/geometry fixes.
  - `examples/iced_chat/card_shell.rs` — `compact_header(bool)` mode,
    content-driven list bodies.
  - `examples/iced_chat/quick_actions.rs` — content-width column rule
    (4/2/1), content-driven card heights.
  - `examples/iced_chat/ui_components.rs`, `boru_dialog.rs`,
    `form_components.rs`, `icon_system.rs`, `log_viewer.rs`,
    `presentation.rs`, `shared_by_me_table.rs`, `sharing_summary.rs`,
    `component_gallery.rs`, `terminal_view.rs` — TypeRole adoption and
    typography alignment.
  - `examples/iced_chat/mcp_server.rs` — one gated test-harness action
    (`boru_gui_clear_mesh_events`, `--enable-gui-test-actions` only).
  - `Cargo.toml` / `Cargo.lock` — boru version 0.116.1 → 0.117.0 + lockfile
    sync (no dependency-graph change beyond the version).
  - `.version-state.json`, `motd.txt` — version/MOTD updates tied to the
    0.117.0 bump.

- **Removed:** No production module was removed. The previously orphaned
  tracked modules `dashboard.rs` / `file_library.rs` / `invitation_qr.rs`
  (no `mod` declaration, dead code) remain tracked until follow-up
  t_8834836b removes them.

- **Shared components or tokens introduced:** `fonts::TypeRole` (15 semantic
  roles with family/weight/size/fallback), `design_tokens::home_content_width`
  + the six content-width breakpoints, `card_shell::compact_header`,
  content-width-aware `quick_action_grid` / `grid_columns_for`, and the
  shared spacing/radius/shadow token tables documented in `DESIGN_SYSTEM.md`.

---

## FONT ROLES

All five families are bundled at compile time via `include_bytes!` in
`fonts.rs` and registered by `fonts::load_fonts()` — no remote font service
at runtime.

| Family | Weights (static, registered) | Roles |
|---|---|---|
| **Source Sans 3** | 400, 500, 600, 700 | Default general UI font — page/section/card titles, body, buttons, supporting text, metadata, fallback for every role |
| **Manrope** | 600, 700 | Major headings only — `display_heading` (hero greeting, 32 px Bold) |
| **Figtree** | 400, 500, 600 | Chat messages, sender labels, message metadata, composer text (`chat_message`, `chat_sender`, `chat_metadata`, `composer_text`) |
| **Raleway ExtraBold** | 800 | BORU wordmark only (`brand_wordmark`, 28 px ExtraBold) |
| **JetBrains Mono** | 400, 500 | Genuine technical identifiers only (`technical_value` — peer IDs, hashes, ports, fingerprints; 12 px Regular) |

- **Font asset and licence records:** `examples/iced_chat/fonts/`
  `THIRD_PARTY_NOTICES.md` records the exact source and version of every
  bundled family, all licensed under SIL OFL-1.1, with the full licence text
  stored beside the assets:
  - Source Sans 3 — v3.052, adobe-fonts/source-sans release (`SourceSans3-OFL.txt`)
  - Manrope — v4.504, google/fonts variable `Manrope[wght].ttf`; statics
    generated with fontTools `varLib.instancer` (OFL-1.1-permitted)
    (`Manrope-OFL.txt`)
  - Figtree — v2.001, erikdkennedy/figtree (`Figtree-OFL.txt`)
  - Raleway — v4.026, google/fonts ofl/raleway (`Raleway-OFL.txt`)
  - JetBrains Mono — v2.211, official JetBrains variable font; statics
    instancer-generated (`JetBrainsMono-OFL.txt`)
  - Inter + combined `OFL.txt` retained for legacy compatibility.
  All 26 font files and 8 licence files are tracked in git. No font files are
  distributed in this report or in the evidence set (plan constraint).

---

## FUNCTIONAL VERIFICATION

UI-HOME-17 action matrix (`docs/ui-redesign/evidence/t_17a358c8/test_matrix.txt`,
OCR-verified captures): **11/11 OK**

| Action | Expected | Result |
|---|---|---|
| Create Public Room | Create Room dialog | OK |
| Create Group Chat | Create Group dialog | OK |
| Add Friend | Friend Requests screen | OK |
| Share Files | Files I'm Sharing (MCP fixture path; native GTK picker not renderable headless — same boundary as UI-HOME-06) | OK |
| Peer row → Chat | End-to-end encrypted conversation | OK |
| Online Peers View all | Friend Requests | OK |
| Mesh View details | Connection Details | OK |
| Tunnels Create tunnel | Create Tunnel dialog | OK |
| Ctrl+N | Create Room dialog | OK |
| Auto-focus name input | typed text lands in Room Name | OK |
| Tab order (name → description) | focus moves to Description | OK |

**Live updates (production paths, OCR-verified):** peer-count badge
0/2 → 1/2 → 0/2 on presence flips; Online Peers rows appear/disappear;
Recent Activity lines ("Ada came online/offline just now") appended with the
bounded event log growing 4 → 5 → 6; Mesh Health status row, stat tiles and
Recent events feed update from real backend state; share-file registration
row appears via the real `SharedFilePicked → file-registration` path. All
driven through unchanged dependency slices — no payload or state semantics
changed.

---

## TESTS

- **Build:** `cargo build --example boru --features gui` → exit 0 at close-out
  HEAD (207 pre-existing warnings, unchanged — approved difference #5).
- **Unit:** Font registration suite 14/14 (part of the GUI example suite);
  `cargo test --lib` → 1824 pass / 20 pre-existing failures, proven unrelated
  (empty `src/` diff vs origin/main; triage ticket t_869b59bf).
- **UI:** `cargo test --example boru --features gui` → **896/896 pass,
  0 failed, 0 ignored** (87.7 s at close-out HEAD 7bc0b5c5; gate reported
  896/896 at 9d34a33c). Per-card counts grew monotonically from 835
  (UI-HOME-02) to 896 (UI-HOME-16..18), consistent with no test loss.
- **Integration:** covered by the GUI suite (creation-flow regressions,
  MCP dispatch, live-update checks) and the 11-row action matrix; network
  layers unchanged.
- **Accessibility:** UI-HOME-18 checklist (`evidence/t_266bfba3/accessibility_checklist.md`)
  — focus rings on focused TextInputs PASS (2 px `color_focus` border),
  focus-ring contrast PASS (≥ 3:1), Ctrl+N / autofocus / Tab order PASS,
  contrast computed from token hexes (two misses filed as follow-ups), iced
  0.14 button keyboard-focus limitation documented (follow-up t_d849e063
  re-evaluated and closed — see REMAINING DIFFERENCES).
- **Platforms:** Linux (Xvfb headless captures; GTK backend). Dark theme out
  of scope (approved difference #6).

---

## VISUAL EVIDENCE

All committed under `docs/ui-redesign/evidence/` on origin/main.

- **Wide screenshot (1600×900):** `t_266bfba3/t_266bfba3_home_1600x900.png`
  (+`_scrolled`, `_grid`, `_scrolled_series/`), `t_dfe40e9f/t_dfe40e9f_home_1600x900.png`
- **Medium screenshot (1280×800):** `t_266bfba3/t_266bfba3_home_1280x800.png`
  (+scrolled/grid/series), `t_dfe40e9f/t_dfe40e9f_home_1280x800.png`,
  `t_4186e7f9/t_4186e7f9_home_populated_1280x800.png`
- **Narrow screenshot (1024×720):** `t_266bfba3/t_266bfba3_home_1024x720.png`
  (+scrolled/grid/series), `t_dfe40e9f/t_dfe40e9f_home_1024x720.png`
- **Minimum-width screenshot (800×600):** `t_266bfba3/t_266bfba3_home_800x600.png`
  (+scrolled/grid/series), `t_dfe40e9f/t_dfe40e9f_home_800x600.png`
- **Approved-mockup comparison:** mockup
  `ui-11/target-figure3.png`; side-by-side composite
  `t_266bfba3/t_266bfba3_side_by_side_mockup_vs_current_1280x800.png`
  (mean abs RGB diff 13.42, independently re-measured at the gate); layout
  structure matches at 1600 and 1280 (sidebar → header → hero → Mesh Health →
  quick actions; rail = Online Peers / Recent Activity / Tunnels).
- **Supporting evidence:** quick-action grid crops
  `t_266bfba3/crops/qa_1280_grid.png` (2×2) + `qa_1600_grid.png` (4-col) with
  all four descriptions OCR-verified; empty-state set `t_4186e7f9/` (10 PNGs);
  typography gallery `t_318cd671/t_318cd671_typography_preview*.png` +
  `t_266bfba3/t_266bfba3_font_gallery_{top,fallback}.png`; overflow OCR
  `geometry.txt` / `verify_1280_wordmap.txt` (`words_past_right_edge = 0`);
  clip gate `quick_action_clip_check.txt` (PASS).

---

## REMAINING DIFFERENCES OR FOLLOW-UPS

Approved differences (UI-HOME-19 gate §6) with follow-up ticket status at
close-out:

1. **text_muted contrast 3.04:1** below WCAG AA 4.5 for 12 px metadata —
   matches mockup style; ticket **t_b2ac1e1a** (`running`).
2. **primary-button white label 4.28:1** misses AA normal text (passes AA
   large 3:1) — ticket **t_80938852** (`running`).
3. **iced 0.14 buttons not keyboard-focusable** — framework limitation
   (no `Status::Focused`); TextInput focus verified. Ticket **t_d849e063**
   re-evaluated the limitation on iced 0.14 and is **done** (commit 7bc0b5c5,
   `docs/ui-redesign/UI-HOME-followup-button-focus.md`).
4. **20 pre-existing `cargo test --lib` failures** — unrelated (empty `src/`
   diff); timestamp-group remediation t_99573d95 / t_42beb205 archived/done;
   triage ticket **t_869b59bf** (`blocked`, rust-dev).
5. **207 pre-existing build warnings** — unchanged, non-blocking, no ticket.
6. **Dark theme** — out of scope (light default; dark tokens covered by
   prior cards).
7. **Mockup REQUESTS rail card** — routes to the dedicated Friend Requests
   screen (verified working); decision ticket **t_89ce5d63** is **done**.
8. **Quick-action copy** — plan card spec wins over Figure 3 text.
9. **Mockup simplified illustration + bottom status strip** — production
   equivalents in the mesh card stat tiles and per-conversation status lines;
   no static mock content in production.
10. **Orphaned tracked modules** dashboard.rs / file_library.rs /
    invitation_qr.rs + optional TypeRole adoption in connection_details.rs /
    download_progress_view.rs — ticket **t_8834836b** (`running`).

No blocking differences remain; all epic DoD items are satisfied by the
verified evidence chain above.

---

## Close-out verification performed (this task)

1. Board audit: all 19 UI-HOME-* cards `done` with committed evidence dirs;
   no blocked/review/verification states in the epic chain.
2. `git fetch origin` + `git log origin/main --oneline -30` — UI-HOME-01…19
   commits (bc5a949e…874ec264) plus the t_d849e063 follow-up (7bc0b5c5) all
   present on origin/main; nothing was missing, so the close-out did not need
   to push any missed UI-HOME code.
3. Working tree check: at session start `git status` was clean. During the
   close-out, three UI-HOME *follow-up* tasks (t_b2ac1e1a, t_80938852,
   t_8834836b) were concurrently running in the same shared directory
   workspace, so the tree at completion contained their in-flight edits
   (connection_details.rs, download_progress_view.rs, DESIGN_SYSTEM.md,
   two docs, one evidence file, one untracked follow-up report). None are
   UI-HOME-01..19 epic leftovers and none were touched by this close-out.
4. Git-trail incident (transparent record): the report commit 1a0bc8b3
   initially swept in three pre-staged deletions of orphaned modules
   (dashboard.rs / file_library.rs / invitation_qr.rs) that belonged to the
   concurrently running t_8834836b task. The close-out caught this
   immediately and restored the files byte-identical in 85f040a7
   ("Revert accidental deletion of orphaned modules…"); the modules remain
   tracked on origin/main for t_8834836b to remove with its own commit.
   The close-out therefore clobbered no other worker's work.
5. Fresh verification at close-out HEAD (7bc0b5c5 tree + follow-up parent
   7b012e81): build exit 0, GUI tests 896/896 pass (87.7 s).
6. Font licence records and TypeRole registration re-verified from the
   source (`fonts.rs`, `THIRD_PARTY_NOTICES.md`).
