# PAPIRUS-20 — Visual QA Report

Task: t_38066b46 · Branch: wt/t_38066b46 · 2026-08-07

## 1. What was captured

Screenshots live in `docs/file-type-icons/PAPIRUS-20-evidence/` (committed with this report).

| File | Surface | Notes |
|---|---|---|
| `t_38066b46_shared_by_me_1440x1500.png` | Shared by Me, full list (17 fixtures) | Primary evidence; every file category in one shot |
| `t_38066b46_shared_by_me_1440x900_v4.png` | Shared by Me @ 900px | Same list, standard window |
| `t_38066b46_shared_by_me_1280x800.png` | Shared by Me @ 1280x800 | Smaller window, layout holds |
| `t_38066b46_shared_by_me_dark_1440x1500.png` | Shared by Me, dark theme | Dark renders icons correctly |
| `t_38066b46_dashboard_downloading_1440x900_v4.png` | Downloading tab (light) | Empty state (no live peer) |
| `t_38066b46_dashboard_downloaded_1440x900_v4.png` | Downloaded tab (light) | Empty state |
| `t_38066b46_dashboard_shared_with_me_1440x900_v4.png` | Shared with Me tab (light) | Empty state |
| `t_38066b46_dashboard_activity_1440x900_v4.png` | Activity Log tab (light) | Empty state |
| `t_38066b46_dashboard_{tab}_dark_1440x900_v4.png` | All four dashboard tabs (dark) | Empty states |

Dashboard tabs with real rows (Downloading with an in-progress transfer, Downloaded with
completed rows, Shared with Me, Activity Log, Peers Downloading from Me, video-card
thumbnails) **could not be captured with live data** — a real transfer requires a second
connected peer, which is out of reach for a headless single-instance capture. The empty
states above prove the surfaces render, but the user should eyeball those rows in a live
two-peer session. Same for the developer component gallery (`Ctrl+Shift+G`): the keypress
does not reach the window under Xvfb, so the gallery screenshot shows the home screen
instead; the gallery is debug-only and not a user-facing surface.

## 2. Visual findings (pixel-verified)

Icons are rendered as crisp vector SVGs — no stretching, no blur, consistent size, and the
file-type icon does not move or resize when filenames are long (the long PDF name is
clipped with an ellipsis while the icon stays fixed at the left of the row). Dark theme
keeps the same icon set with good contrast. **No legacy emoji** appear anywhere in the
file-sharing views.

| Category | Fixture | Icon shown in Shared by Me | Correct? |
|---|---|---|---|
| PDF | QuarterlyReport.pdf / long-name PDF | Red Papirus PDF icon (pixel-verified) | ✅ |
| Image | vacation-photo.jpg, screenshot.png | Coloured Papirus image icon | ✅ |
| Word | MeetingNotes.docx | **Grey generic octet-stream icon** | ❌ |
| Spreadsheet | Budget2026.xlsx | **Grey generic octet-stream icon** | ❌ |
| Presentation | Roadmap2026.pptx | **Grey generic octet-stream icon** | ❌ |
| Video | demo-landscape.mp4, demo-vertical.mp4 | **Grey generic octet-stream icon** | ❌ |
| Audio | interview-recording.mp3 | **Grey generic octet-stream icon** | ❌ |
| Archive | source-bundle.zip, archive.7z | **Grey generic octet-stream icon** | ❌ |
| Source | main.rs, script.py | **Grey generic octet-stream icon** | ❌ |
| Unknown | mystery.crypt | Grey generic octet-stream icon | ✅ (unknown is generic) |
| Text/Markdown | notes.txt, README.md | Text icons | ✅ |

The red PDF icon was confirmed by pixel scan of the row region (RGB ≈ (192,32,32)); the
image rows show saturated orange/blue and green icon colours; all other rows have no
saturated icon pixels — they render the grey `application-octet-stream` generic icon.

## 3. Root cause of the mismatched icons (code-verified)

`AppMessage::SharedFilePicked` (`examples/iced_chat/app.rs:19881-19898`) derives the stored
MIME type from a tiny extension map that only covers
`txt, md, json, pdf, png, jpg/jpeg, gif, webp`. Every other extension
(`docx, xlsx, pptx, mp4, mp3, zip, 7z, rs, py, …`) is stored as
`application/octet-stream`. This map predates the PAPIRUS work (commit 02fac771) and the
pre-PAPIRUS table also rendered those rows with a generic `Icon::Files`, so this is not a
PAPIRUS regression — but it means the resolver never gets the chance to use its rich
extension table for these files.

The resolver (`examples/iced_chat/file_type_resolver.rs`) *does* contain exact entries for
all these extensions (`.xlsx` → spreadsheet, `.docx` → document, `.mp4` → video, etc.), and
the priority chain places an advertised MIME (`application/octet-stream`, priority 4) above
the ordinary filename extension (priority 6). Because the share path stamps
`application/octet-stream` as the advertised MIME, resolution short-circuits to the generic
icon and the extension is never consulted.

The inconsistency is cross-surface: chat file cards and video cards call
`file_type_icon_element_with_tooltip(&name, None, None, …)` (download_progress_view.rs:705,
video_file_card.rs:773), passing **no** MIME, so the resolver falls through to the extension
and renders the correct video/PDF/archive icon for the *same file* that shows a generic icon
in the Shared by Me table. This violates acceptance criterion 6 ("Chat attachments, video
cards, dashboard rows and activity records are consistent") and the Task-20 check "the same
type uses the same icon everywhere" — for every extension outside the 8-entry map.

## 4. Code-level consistency verification

- **Single resolver, single component:** every file-sharing surface routes through
  `file_type_icon_element` / `decorative_file_type_icon_element` /
  `file_type_icon_element_with_tooltip` / `directory_icon_element`
  (download_progress_view.rs), which funnel into `FileTypeIcon::new` →
  `resolve_file_icon` sharing one `FILE_TYPE_ICON_CACHE`. Grepped across
  shared_by_me_table.rs, download_progress_view.rs, video_file_card.rs, component_gallery.rs,
  app.rs dashboard rows. ✅
- **Transfer status is separate from the type icon:** state badges (Downloading/Downloaded/
  Failed) are rendered as pill badges beside the icon; no surface recolours the type icon to
  signal status. ✅ (verified in code; live-state rows not captured visually)
- **No legacy emoji / old icon constants:** grep for emoji literals and the old
  per-screen MIME→`Icon::Image/Play/Files` map (removed by PAPIRUS-11) finds no remaining
  call sites; the pre-PAPIRUS `file_icon()` helper is gone. ✅
- **Folder rows:** no production surface currently renders a folder row — folder sharing is
  an explicit limitation (`SharedFolderPicked` surfaces a message; `directory_icon_element`
  is `#[allow(dead_code)]`, used only by unit tests). The Papirus folder icon exists in the
  bundle and is unit-tested (PAPIRUS-12), but there is no shared-folder row to photograph.
- **Build gate:** `rb check --bin boru --features gui,video-playback,terminal` on debsrv
  exits 0 (216 pre-existing warnings). ✅

## 5. Residual / follow-up

- **Residual (documented):** Office/video/audio/archive/source files shared through the UI
  show the generic octet-stream icon in Shared by Me (and by extension any dashboard row
  fed by the same stored MIME), instead of their type-specific Papirus icon. Root cause:
  the 8-entry MIME map in `SharedFilePicked` + the resolver trusting advertised MIME over
  the extension. Same file shows the correct icon in chat cards. → Follow-up task created.
- **Not captured with live data:** Downloading/Downloaded/Shared-with-me/Activity rows,
  Peers Downloading from Me, video-card thumbnails, component gallery. Empty states
  captured; user should eyeball live two-peer sessions.

## 6. Fixtures / harness

Fixtures in `/tmp/papirus-qa-fixtures-1227043/` (built for this task). Capture harnesses:
`/tmp/papirus20_capture_v3.sh` (first pass) and `/tmp/papirus20_capture_v4.sh`
(final; registers fixtures newest-first so every category appears in the top rows of the
newest-first list, uses a 1440x1500 window on a 1600px Xvfb screen). Launched with
`BORU_PAPIRUS_ASSETS` pointing at the worktree's Papirus bundle so real SVG assets render.
