# PAPIRUS-21 — Octet-stream MIME fix: consistent icons across all surfaces

Task: t_9d6e0d40 · 2026-08-07

## 1. Problem

Files shared through the UI with extensions outside the legacy 8-entry map
(`{txt, md, json, pdf, png, jpg/jpeg, gif, webp}`) were stored with MIME
`application/octet-stream` by `AppMessage::SharedFilePicked`
(`examples/iced_chat/app.rs`). Every dashboard surface (Shared by Me,
Downloading, Downloaded, Shared with Me, Activity) feeds the stored MIME
into the resolver's **advertised** slot (priority 4), which outranked the
filename extension (priority 6). Result (pixel-verified in PAPIRUS-20): an
`.xlsx/.docx/.pptx/.mp4/.mp3/.zip/.7z/.rs/.py` file showed the grey generic
octet-stream icon in Shared by Me while the same file showed the correct
type icon in chat cards (which pass `None` MIME and fall to the extension).

## 2. Fix (both halves of the task's preferred approach)

### 2a. Resolver: `application/octet-stream` is "no MIME info"
`examples/iced_chat/file_type_resolver.rs`

- New `MIME_NO_INFO` constant + module-doc section documenting the rule.
- `mime_lookup` now returns `None` for `application/octet-stream`, so it
  never wins at priorities 2–4 and the filename extension (priority 6) wins
  whenever one exists. This fixes **legacy rows** already stored with
  octet-stream, peer-advertised octet-stream, and every dashboard row that
  feeds stored MIME, with one change.
- Because `mime_category_hint` and `detect_mismatch` build on `mime_lookup`,
  octet-stream yields no category hint (unknown `.crypt` still ends on the
  generic `application-x-generic` at priority 8) and never triggers a false
  MIME-mismatch warning.
- `resolve_cache_key` treats octet-stream as absent (`meaningful_mime`), so
  it shares a cache entry with the no-MIME case.
- The `application-octet-stream` icon stays in the bundle and remains
  reachable via the `.bin` extension entry.

### 2b. Stamping: expanded MIME map in `SharedFilePicked`
`examples/iced_chat/app.rs` (~19881)

- The 8-entry extension→MIME match now covers the extensions the central
  resolver knows (Documents, Spreadsheets, Presentations, Images, Video,
  Audio, Archives, Source code, Executables/installers/disk images,
  Databases/fonts/keys, Ebooks/torrents/3D), using the exact MIME strings
  the resolver maps — so newly shared files store a real MIME, benefit any
  consumer of stored MIME, and show the same icon as chat cards.
- `.h`/`.hpp`, certificates, and a few disk-image formats are deliberately
  left on octet-stream where the standard MIME would map to a different
  icon than the extension path; the resolver fall-through keeps them
  identical to chat.
- Unknown extensions still fall back to `application/octet-stream` (now
  harmless: the resolver treats it as no-info).

No protocol/transfer/payload changes. No Papirus bundle/manifest changes.
No UI redesign. No legacy emoji/icons introduced.

## 3. Tests

`examples/iced_chat/file_type_resolver.rs` — new PAPIRUS-21 section (7 tests):

- `octet_stream_advertised_falls_through_to_extension` — docx/xlsx/pptx/
  mp4/mp3/zip/7z/rs/py with octet-stream resolve to the exact chat-card
  icon via the extension (asset path asserted identical to the no-MIME
  resolution).
- `octet_stream_locally_detected_falls_through_to_extension` — legacy
  stored octet-stream in the local slot also falls through.
- `octet_stream_with_unknown_extension_stays_generic` — `mystery.crypt` +
  octet-stream → `application-x-generic`, identical to no-MIME.
- `octet_stream_with_bin_extension_resolves_binary_icon` — `.bin` keeps the
  octet-stream binary icon via the extension table.
- `octet_stream_without_extension_falls_back_to_unknown_generic`.
- `octet_stream_never_triggers_mime_mismatch`.
- `octet_stream_cache_key_equals_absent`.

The old `known_mime_resolves_exact_icon` case that pinned octet-stream →
`AdvertisedMime` was updated (octet-stream no longer wins at priority 4);
all other existing tests are untouched.

### Results (debsrv via `rb`, canonical repo)

| Run | Result |
|---|---|
| `rb test --example boru --features gui,video-playback,terminal -- file_type_resolver` | 65 passed, 0 failed (incl. all 7 new + all PAPIRUS-19 `task19_*`) |
| `rb test --example boru --features gui,video-playback,terminal -- file_type_icon` | 39 passed, 0 failed |
| `rb check --example boru --features gui,video-playback,terminal` | exit 0 (216 pre-existing warnings, same as PAPIRUS-20 baseline) |

## 4. Visual verification (headless Xvfb + MCP, PAPIRUS-20 harness pattern)

Re-captured the Shared by Me table with the fixed binary (`BORU_PAPIRUS_ASSETS`
pointing at the canonical bundle; fixtures registered through the production
`boru_gui_test_share_file` → `AppMessage::SharedFilePicked` path, so the new
stamping map is exercised end to end). Screenshots in
`docs/file-type-icons/PAPIRUS-21-evidence/`:

- `t_9d6e0d40_shared_by_me_1440x1800.png` (light, all 16 rows)
- `t_9d6e0d40_shared_by_me_1440x900.png` (light, standard window)
- `t_9d6e0d40_shared_by_me_dark_1440x1800.png` (dark, all 16 rows)

Pixel-verified (connected-component scan of the icon column; dominant RGB of
the saturated icon pixels; same result in light and dark):

| Row | Expected | Light RGB | Dark RGB | Result |
|---|---|---|---|---|
| Budget2026.xlsx | green spreadsheet | (74,172,78) | (74,171,78) | ✅ |
| MeetingNotes.docx | blue Word | (39,138,212) | (38,136,210) | ✅ |
| Roadmap2026.pptx | orange PowerPoint | (250,90,40) | (251,89,38) | ✅ |
| QuarterlyReport.pdf | red PDF | (194,63,57) | (192,59,53) | ✅ |
| vacation-photo.jpg | image thumbnail | (204,141,57) | (212,140,47) | ✅ |
| screenshot.png | image thumbnail | (72,143,206) | (63,143,216) | ✅ |
| demo-landscape.mp4 | blue video | (114,130,216) | (113,129,215) | ✅ |
| demo-vertical.mp4 | blue video | (114,130,216) | (113,129,215) | ✅ |
| interview-recording.mp3 | orange audio | (250,153,11) | (251,152,8) | ✅ |
| source-bundle.zip | green archive | (76,172,80) | (75,172,79) | ✅ |
| archive.7z | green archive | (76,172,80) | (75,172,79) | ✅ |
| main.rs | orange source | (218,108,49) | (218,106,47) | ✅ |
| script.py | python icon | python logo mark | python logo mark | ✅ |
| **mystery.crypt** | **grey generic** | **no saturated pixels** | **no saturated pixels** | ✅ |
| notes.txt | grey text page | grey (by design) | grey | ✅ |
| README.md | red markdown accent | (215,82,82) | (215,80,80) | ✅ |

The stored-MIME metadata lines also now show the real type
(`application/vnd.openxmlformats-officedocument.spreadsheetml.sheet`,
`video/mp4`, `audio/mpeg`, `application/zip`, …) instead of
`application/octet-stream` for every category except the unknown
(`mystery.crypt` correctly keeps octet-stream → grey generic).

## 5. Downloaded / Activity rows

The Downloaded/Shared-with-me/Activity rows use the same central
`decorative_file_type_icon_element(&name, item.mime_type, None, …)`
(app.rs dashboard rows) — the stored MIME feeds the same advertised slot, so
both halves of the fix apply identically: newly shared files store a real
MIME (2b) and any octet-stream value is treated as no-info (2a). Live
transfer rows still need a two-peer session to photograph (same limitation
as PAPIRUS-20); the unit tests cover the octet-stream advertised/local
slots that those rows exercise.

## 6. Files changed

- `examples/iced_chat/file_type_resolver.rs` — octet-stream-as-no-info rule
  + 7 new tests (+1 updated case).
- `examples/iced_chat/app.rs` — expanded SharedFilePicked MIME map.
- `docs/file-type-icons/PAPIRUS-21.md` — this report.
- `docs/file-type-icons/PAPIRUS-21-evidence/` — 3 screenshots.

Production lines changed: 2 files (resolver + stamping map). No bundle,
manifest, protocol, or UI changes.
