# UI-HOME-10 evidence (task t_faa09772)

Overflow / clipping / scroll audit evidence.

## `after/` — main evidence (scripts/ui_home10_evidence.sh after)

| File | State | What it proves |
|---|---|---|
| `home_longname_1280x800_after.png` | seeded friends (incl. `a-very-long-display-name-for-truncation-test-peer-42`) + long local label via `--name` | Online Peers long name wraps to two lines (row grows); Recent Activity descriptions wrap, no 40-char truncation; greeting wraps; rightmost word x≈1241 < 1280 (no horizontal overflow) |
| `home_narrow_1024x720_after.png` | narrow window | rail stacks below 1120 px; rightmost word x≈981 < 1024 (no horizontal scrollbar); no clipped text |
| `geometry_after.txt` | OCR TSV geometry | y-positions of the wrapped long-name rows |

## `scroll/` — vertical page scroll proof (scripts/ui_home10_scroll_proof.sh)

| File | What it proves |
|---|---|
| `home_scroll_top_900x650.png` | top of the page at a short window: greeting + hero |
| `home_scroll_bottom_900x650.png` | after wheel-scroll: RECENT ACTIVITY + Online Peers rows visible — the page scrolls vertically and content below the fold is reachable; rightmost word x≈846 < 900 |

Harness prerequisites: built GUI binary (`target/debug/boru`),
Xvfb, xdotool, ImageMagick `import`, tesseract.
