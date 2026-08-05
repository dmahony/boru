# UI-HOME-09 evidence — spacing / hierarchy / vertical rhythm

Task `t_a24fbc67`. Before/after full-page captures at the two required
widths plus a populated state, and OCR word-box geometry files used to
verify the numeric spacing changes.

| File | What it shows |
|---|---|
| `before/home_1600x900_before.png` | Wide 1600×900, fresh launch (empty rail), pre-change |
| `before/home_1280x800_before.png` | Reference 1280×800, fresh launch (empty rail), pre-change |
| `before/home_populated_1280x800_before.png` | 1280×800, seeded fixture + real presence events, pre-change |
| `after/home_1600x900_after.png` | Wide 1600×900, fresh launch, post-change |
| `after/home_1280x800_after.png` | Reference 1280×800, fresh launch, post-change |
| `after/home_populated_1280x800_after.png` | 1280×800, seeded fixture + real presence events, post-change |
| `side_by_side_1280x800.png` | before \| after composite at the reference width |
| `before/geometry_before.txt` | tesseract TSV header y-positions, pre-change |
| `after/geometry_after.txt` | tesseract TSV header y-positions, post-change |

## Verified numeric deltas (1280×800, OCR geometry)

| Landmark | Before | After | Delta | Source change |
|---|---|---|---|---|
| Rail card top edge (Online Peers) | 109 | 119 | +10 | greeting gap 2→4 (+2) + header→dashboard 20→28 (+8) |
| ONLINE PEERS header | 138 | 148 | +10 | page-header shift |
| RECENT ACTIVITY header | 363 | 383 | +20 | +10 page shift + 10 header→content |
| TUNNELS header | 489 | 523 | +34 | +10 page shift + 10 (peers) + 14 (activity: 10 header→content + 4 empty-state) |
| Mesh Health header | 361 | 371 | +10 | page-header shift |

Both columns' first card tops sit at the same y (109 / 119) → edges align.
All deltas match the intended shared-scale token changes; no content moved
otherwise.
