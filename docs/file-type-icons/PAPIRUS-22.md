# PAPIRUS-22 — Shared by Me row layout: icon aligned with the file entry

Task: t_28d7484a · Branch: wt/t_28d7484a · 2026-08-07

## 1. Problem

In the "Files I'm Sharing" table the file-type icon appeared pushed down
relative to its file entry: the filename ("file entry") looked pushed too
high.  Pixel measurement of the PAPIRUS-21 capture
(`PAPIRUS-21-evidence/t_9d6e0d40_shared_by_me_1440x900.png`) showed why:

- the metadata line is `kind_label(mime) · size · shared X`, where
  `kind_label` returns the **full MIME string** (e.g.
  `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet`);
- the name column is only ~120–160 px wide, so that line **wraps to 2–4
  visual lines** for essentially every row (even `text/plain · 19 B · shared
  23s ago` wrapped);
- the name cell was `Row[icon][space][name_block]` with
  `align_y(Alignment::Center)`, so the 32 px icon was vertically centred on
  the **whole multi-line block** and landed below the filename, beside the
  wrapped metadata lines — e.g. Budget2026.xlsx: filename at y≈305–317,
  icon tile at y≈335–365 (tile centre ~24 px below the filename centre);
- the wrapped metadata visually collided with the icon's horizontal band
  (text fragments above and below the tile).

The `app.rs` catalogue row has the same `[icon][space][two-line column]`
pattern but truncates the MIME to 18 chars, so it never wraps — the Shared
by Me table was the only surface missing that protection.

## 2. Fix

`examples/iced_chat/shared_by_me_table.rs`, `name_cell`:

- the icon now sits in its own row **beside the filename** and is vertically
  centred on it (`Row[icon][space][filename]`, `align_y(Center)`);
- the metadata sub-line hangs **below** the pair, spanning the full name
  column, so a long MIME can wrap on its own lines below the icon instead of
  wrapping beside it and pushing the entry up;
- nothing else changed (same Papirus icon component/resolver, same data).

Resulting row structure:

```
[icon] Budget2026.xlsx
       application/vnd.openxmlformats-officedocument.spreadsheetml.sheet ·
       3.6 KiB · shared just now
```

## 3. Verification (pixel-measured, light + dark, 1440×1800)

Evidence: `PAPIRUS-22-evidence/` (captured with the fixed binary via the
production `SharedFilePicked` path, 16 fixtures, `BORU_PAPIRUS_ASSETS` set).

| Rows | Icon-vs-filename centre delta |
|------|-------------------------------|
| 10 of 14 (all single-line filenames) | **≤ 1.5 px** (icon centred on the filename) |
| 4 of 14 (filenames ≥ ~17 chars that wrap: demo-landscape.mp4, demo-vertical.mp4, vacation-photo.jpg, long PDF name) | ~10–11 px — icon centred on the **2-line wrapped name block** (first line 11 px above centre, second 11 px below); full name available via the existing hover tooltip |

Before the fix every row measured 20–40 px of offset (icon centred on the
multi-line metadata block, filename above the tile).  After the fix no row
has metadata text running beside/under the icon — the metadata starts below
the icon+filename line.  Identical results in dark theme.

Other checks:

- `cargo test --bin boru --features gui,video-playback,terminal`:
  **1077 passed, 0 failed** (incl. 14 `shared_by_me_table` tests).
- `rb check --bin boru --features gui,video-playback,terminal`: exit 0
  (216 pre-existing warnings, unchanged).
- No icon behaviour changed: same central `FileTypeIcon` component, same
  resolver, same sizes; Papirus icons verified in the captures (PDF red,
  DOCX blue, XLSX green, PPTX orange, video/audio/archive/source type icons,
  mystery.crypt generic grey).

## 4. Out of scope / residual

- Filenames ≥ ~17 chars wrap inside the ~160 px name column (pre-existing;
  the hover tooltip exposes the full name).  Widening the name column or
  width-aware truncation of filenames would be a separate change.
- The metadata still shows the full MIME string and wraps below the entry;
  shortening/truncating the kind label was left out of scope per the task.
