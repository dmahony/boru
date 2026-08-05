# UI-HOME-10 — Overflow / clipping / scroll audit

- Task: `t_faa09772` (UI-HOME-10)
- Plan source: `Boru_Home_Screen_Tidy_and_Fonts_Hermes_Kanban_Plan.pdf` (UI-HOME-10 card)
- Repo: `/home/dan/iroh-gossip-chat` @ main (based on `a8aaa831` UI-HOME-09)
- Status: DONE (build green, 884/884 tests pass, long-name + narrow evidence captured, removed constraint list, pushed)
- Labels: ui-home, responsive, regression-risk, accessibility
- This card (with 04–14) gates UI-HOME-15.

## Summary

Dedicated overflow/clipping audit over the whole home component tree. Every
remaining hidden-overflow mask on the home screen is removed:

1. **Rail-card rows are now content-driven** — Online Peers (was a fixed
   60 px button height), Recent Activity (was a fixed 32 px row) and Tunnels
   (was a fixed 48 px row) all grow with wrapped content. The approved row
   rhythm is preserved as a MINIMUM via a zero-width min-height spacer, not
   as a fixed box, so a long display name / endpoint / description wraps to
   two lines and the row grows instead of clipping.
2. **Hidden-overflow masks removed** — Recent Activity and Mesh-event
   descriptions were truncated to 40 chars, forced `Wrapping::None` and
   clipped (`container.clip(true)`). All three are gone: descriptions now
   render in full and wrap naturally.
3. **Long technical identifiers wrap at glyph level** — peer-key-style
   display names, JetBrains Mono tunnel endpoints, the greeting, the hero
   headline and the mesh status/lobby lines now use `Wrapping::WordOrGlyph`
   with `width(Fill)` so an unbroken 64-char peer key wraps inside its row
   instead of overflowing it.
4. **Sidebar identity no longer overflows** — the pinned identity block's
   display name used `Wrapping::None` + width Shrink; a long local label
   could paint over the main panel. It now fills the sidebar width and
   wraps.

The page scrolls vertically (the gutter_scrollable gives its content
infinite height — verified in iced 0.14's scrollable layout), and no
horizontal scrollbar appears (the page scrollable is vertical-only).

## What changed

### `examples/iced_chat/app.rs`

| Constraint | Before | After |
|---|---|---|
| Online Peers row height | button `.height(Fixed(PEER_ROW_HEIGHT))` = 60 px (clips wrapped names) | content-driven with a zero-width 60 px min-height spacer; button height removed |
| Online Peers name | default Word wrapping | `Wrapping::WordOrGlyph` (peer-key names wrap at glyph level) |
| Recent Activity row height | container `.height(Fixed(ACTIVITY_ROW_HEIGHT))` = 32 px (clips wrapped descriptions) | content-driven with a zero-width 32 px min-height spacer |
| Recent Activity description | `truncate_with_ellipsis(..., 40)` + `Wrapping::None` + `.clip(true)` | full description, `Wrapping::WordOrGlyph`, no clip, no truncation |
| Tunnels row height | container `.height(Fixed(CARD_ROW_HEIGHT))` = 48 px (clips wrapped endpoints) | content-driven with a zero-width 48 px min-height spacer |
| Tunnels name/endpoint column | no width Fill; endpoint default Word | `.width(Length::Fill)` + `Wrapping::WordOrGlyph` on name and endpoint |
| Mesh event rows | `truncate_with_ellipsis(..., 40)` + `Wrapping::None` | full message, `Wrapping::WordOrGlyph`, no truncation |
| Greeting (`Good …, {name}`) | default Word | `Wrapping::WordOrGlyph` (long display names / peer keys wrap inside the header) |
| Hero headline | default Word, no width Fill | `.width(Length::Fill)` + `Wrapping::WordOrGlyph` (long degraded/offline reason wraps) |
| Mesh status label/detail | no width Fill on the status column | `.width(Length::Fill)` + `Wrapping::WordOrGlyph` on label and detail |
| Mesh lobby line | no width Fill | `.width(Length::Fill)` + `Wrapping::WordOrGlyph` |
| Sidebar identity name | `Wrapping::None` + width Shrink (overflowed long local labels) | `.width(Length::Fill)` + `Wrapping::WordOrGlyph`, column width Fill |

### Justified constraints kept (not defects)

| Constraint | Why it stays |
|---|---|
| `CardShell` list body `max_height` (180 / 120 / 5-row cap) | Bounded scrollable lists are the documented, intentional rail pattern (card_shell.rs:8–12) — long lists scroll inside the card instead of growing the dashboard unbounded. |
| Sidebar rows `Wrapping::None` + `.clip(true)` + tooltip (>24 chars) | Deliberate single-line sidebar rows with a hover tooltip revealing the full name (UI-18 pattern, accessible, not hidden). |
| Hero illustration fixed 205×140 | Decorative NETWORK_MOTIF SVG, hidden below 640 px; not user content. |
| `HERO_MIN_CONTENT_HEIGHT` spacer | Content-driven minimum (the row grows past it when the headline wraps). |
| `PEERS_BODY_MIN` empty-state floor | Min-height floor, not a clip; content fits within it. |
| Page gutter_scrollable + bounded card scrollables | Intentional nesting: page scrolls vertically; card lists scroll independently inside their caps. |
| Quick-action grid breakpoints | Already content-driven (UI-HOME-06); verified no fixed height / no clip remains. |

## Removed constraint list (required evidence)

1. `view_online_peers_card`: button `.height(Length::Fixed(PEER_ROW_HEIGHT))` removed → zero-width min-height spacer (60 px floor, content growth).
2. `view_online_peers_card`: name text now `Wrapping::WordOrGlyph`.
3. `view_recent_activity_card`: container `.height(Length::Fixed(ACTIVITY_ROW_HEIGHT))` removed → zero-width min-height spacer (32 px floor, content growth).
4. `view_recent_activity_card`: `truncate_with_ellipsis(&event.description, 40)` removed — full description rendered.
5. `view_recent_activity_card`: `.wrapping(Wrapping::None)` → `Wrapping::WordOrGlyph`.
6. `view_recent_activity_card`: description container `.clip(true)` removed.
7. `view_tunnels_card`: container `.height(Length::Fixed(CARD_ROW_HEIGHT))` removed → zero-width min-height spacer (48 px floor, content growth).
8. `view_tunnels_card`: name/endpoint column now `.width(Length::Fill)` (was Shrink, could overflow the row); the redundant `Space::new().width(Length::Fill)` after it removed.
9. `view_tunnels_card`: name and endpoint now `Wrapping::WordOrGlyph` (JetBrains Mono host:port wraps instead of overflowing).
10. `view_chat_list_content` mesh events: `truncate_with_ellipsis(&event.message, 40)` removed — full message rendered.
11. `view_chat_list_content` mesh events: `.wrapping(Wrapping::None)` → `Wrapping::WordOrGlyph`.
12. `view_chat_list_content` greeting: `Wrapping::WordOrGlyph` added.
13. `view_chat_list_content` hero headline: `.width(Length::Fill)` + `Wrapping::WordOrGlyph` added.
14. `view_chat_list_content` mesh status label/detail: `.width(Length::Fill)` + `Wrapping::WordOrGlyph` added to both; status column now width Fill.
15. `view_chat_list_content` mesh lobby line: `.width(Length::Fill)` + `Wrapping::WordOrGlyph` added.
16. `view_local_profile_block` (sidebar identity): `Wrapping::None` + width Shrink removed; name now `.width(Length::Fill)` + `Wrapping::WordOrGlyph`; identity row now `.width(Length::Fill)`.

## Tests

- `cargo build --example boru --features gui` — OK (exit 0; 207 pre-existing warnings unchanged).
- `cargo test --example boru --features gui` — **884 passed / 0 failed** (prior 880; +4 new regression guards).
- New tests (source-inclusion guards matching the existing UI-HOME-09/12 pattern):
  - `home_rail_rows_are_content_driven_not_fixed_height` — forbids fixed 60/32/48 px row heights in the three rail cards; requires the zero-width min-height spacer for each.
  - `home_rail_descriptions_wrap_naturally_not_truncated_or_clipped` — forbids `truncate_with_ellipsis`, `Wrapping::None` and `.clip(true)` in Recent Activity / Mesh events; requires `Wrapping::WordOrGlyph`.
  - `home_long_technical_text_wraps_at_glyph_level` — requires `Wrapping::WordOrGlyph` on peer names, tunnel endpoints and home long text; requires the mesh status detail to be width-Fill.
  - `sidebar_identity_name_wraps_inside_sidebar` — forbids `Wrapping::None` in the identity block; requires WordOrGlyph + width Fill.

## Evidence

`docs/ui-redesign/evidence/t_faa09772/` (capture harness: `scripts/ui_home10_evidence.sh`,
`scripts/ui_home10_scroll_proof.sh`; Xvfb + MCP `boru_gui_navigate` /
`boru_gui_set_peer_presence` + tesseract TSV geometry):

- `after/home_longname_1280x800_after.png` — seeded friends incl. the long
  name `a-very-long-display-name-for-truncation-test-peer-42`, long local
  label passed via `--name`. OCR geometry shows:
  - Online Peers row: `a-very-long-display-` (y=498) + `name-for-truncation-`
    (y=517) — the long peer name wraps to two lines inside one row; the row
    grew past the old fixed 60 px (two 13–15 px lines + presence line no
    longer fit in 60 px).
  - Recent Activity rows: the long description wraps (`a-very-long-display-`
    y=643, `name-for-truncation-` y=662) — no truncation ellipsis.
  - Greeting wraps to 3 lines for the long local label (y=36/75/113).
  - Rightmost OCR word ends at x≈1241 < 1280 — no horizontal overflow.
- `after/home_narrow_1024x720_after.png` — narrow window (rail stacked
  below 1120 px). Rightmost OCR word x≈981 < 1024 — no horizontal
  scrollbar; layout stacks cleanly; no clipped text at the right edge.
- `scroll/home_scroll_top_900x650.png` / `home_scroll_bottom_900x650.png` —
  900×650 short window. Top shows greeting/hero; after wheel-scroll the
  view shows RECENT ACTIVITY + Online Peers rows with the long name — proof
  the page scrolls vertically and content below the fold is reachable.
  Rightmost word x≈846 < 900 after scroll.
- `after/geometry_after.txt` — OCR geometry report.

## Remaining risks / notes

- `cargo fmt` repo-wide still shows pre-existing drift; this card's edited
  regions are rustfmt-clean by inspection.
- 207 pre-existing build warnings untouched (UI-HOME-01 baseline).
- The `truncate_with_ellipsis` helper remains in `presentation.rs` and is
  still used by the File Sharing views (which are outside this card's
  home-tree scope); the home rail no longer uses it.
- Recent Activity rows that wrap to 2+ lines make the bounded card list
  scroll slightly more; this is the intended "cards grow to contain
  content" behaviour and the list cap (max_height 180) still bounds the
  card.
