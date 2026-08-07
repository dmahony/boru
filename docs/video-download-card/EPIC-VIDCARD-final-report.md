# EPIC-VIDCARD — Final Implementation Report

Video download card redesign for Boru chat (iced frontend).
Close-out task: `t_78f5b5d2`. Parent epic: `EPIC-VIDCARD` (created by `t_6e936d77`).
Spec: `Boru_video_download_card_redesign.txt` (attachment of `t_6e936d77`).

---

## 1. Summary of the redesign

The Boru video download card (`examples/iced_chat/video_file_card.rs`) was rebuilt from the
old `download_progress_view` card into a modern, reusable `BoruVideoFileCard` component that
matches the approved Boru design system:

- **Card surface**: soft-white theme-aware surface (dark-mode aware), 1px neutral green-grey
  border, 16px radius, very subtle shadow, 20–24px internal padding, content-driven height,
  `Length::Shrink` capped by the chat column (VIDCARD-03).
- **Header**: compact tinted real-state badge, video icon (lucide `film`), single-line truncated
  filename with full name in tooltip + Copy action, uppercase format label, icon-only overflow
  menu (VIDCARD-04).
- **Media frame**: aspect-ratio-aware sizing (Portrait/Square/Landscape classes), exact intrinsic
  ratio preserved, `ContentFit::Contain` everywhere, identical poster/player geometry, 13px rounded
  dark frame, 64px circular "Play video" overlay, duration badge from real metadata, loading
  indicator, hidden overflow only at the frame boundary (VIDCARD-05..08, 10, 11).
- **Status/metadata**: real state line, `From: <sender>`, size, duration (live player only),
  received/shared time from real timestamps; unavailable values hidden, never invented;
  duplicate filename removed from the surrounding system line (VIDCARD-12).
- **Actions**: state-appropriate green filled primary / light bordered secondary hierarchy;
  Play / Retry / Resume / Download / Open File / Open Folder / Re-share / Pause / Cancel / Remove;
  the old default-styled blue "Open downloads folder" button was removed (VIDCARD-13).
- **Download progress**: thin rounded green progress bar (light + dark themes), fixed-width
  percentage label, real bytes/total/percent/speed detail line, no invented ETA (VIDCARD-14).
- **Responsive**: CardBand Wide/Medium/Narrow derived from measured timeline width; media caps
  scale down, metadata wraps, actions wrap, filename truncates, no horizontal scroll (VIDCARD-15).
- **Poster generation**: post-download ffmpeg/ffprobe pipeline, content-addressed cache,
  off-UI-thread, dimension-bounded (VIDCARD-16, 18).
- **Accessibility**: keyboard-focusable buttons with visible focus ring, accessible names,
  real-text progress value, status not colour-alone, reduced-motion respected (VIDCARD-17).
- **Security/privacy**: all Task 18 guardrails verified; hostile received-posters are
  dimension-bounded before decode (VIDCARD-18).

## 2. Files and components added or modified

Added (new files):
- `examples/iced_chat/video_file_card.rs` — the reusable `BoruVideoFileCard` component
  (781 lines at VIDCARD-02, grown to ~2,800 lines including tests through VIDCARD-19).
- `examples/iced_chat/focusable_button.rs` — keyboard-focusable button wrapper (VIDCARD-17).
- `assets/icons/lucide/film.svg` — video icon (VIDCARD-04).
- `docs/video-download-card/VIDCARD-01-architecture.md` — architecture note (VIDCARD-01).

Modified (examples):
- `examples/iced_chat/app.rs` — attachment model (DownloadAttachment), download action handlers
  (retry/restart/cancel guards), inline-video play/seek, poster-done handling, responsive
  timeline-width threading, thumbnail dimension bounds.
- `examples/iced_chat/download_progress_view.rs` — progress bar / action button helpers,
  delegation to the new card.
- `examples/iced_chat/main.rs` — module declarations.
- `examples/iced_chat/icon_system.rs` — `Icon::Video` registration.

Modified (src/, additive / display-only — see §7):
- `src/video_playback.rs` — additive ffprobe metadata probe (`probe_local_video_metadata`,
  139 insertions, 0 deletions; no existing playback logic changed).
- `src/video_poster.rs` — additive poster bounds helper + guardrail (35 insertions, 1 deletion).
- `src/chat_callbacks.rs` — `set_pending_file` gained an optional `sender_label` param
  (interface addition; all implementors updated).
- `src/chat_core.rs` — system line text only: `"{name} shared a file: {file}"` →
  `"{name} shared a file"` (message payload unchanged).
- `src/system_events.rs` — classifier matches the new display text, backward compatible with
  older persisted lines.
- 18 integration-test targets — updated to the new trait signature (no behavioural changes).

## 3. Existing components reused

- `design_tokens` — `card_style` (surface/border/radius/shadow), SPACE_12/20/24 scale, color
  tokens, focus token.
- `icon_system` (`Icon::Video`, `Icon::Folder`, `Icon::Play`, etc.).
- `ui_components::OverflowMenu` — overflow menu.
- `download_progress_view` — progress/action helper functions reused inside the new card.
- `src/video_poster` — existing poster-generation pipeline (ffmpeg → bounded WebP, cache).
- `src/video_playback::verify_local_attachment` — local-file verification.
- `fonts` — Figtree (chat-facing text), JetBrains Mono (technical identifiers only).
- Existing `ChatCallbacks` interface (extended with an optional param, not replaced).

## 4. Aspect-ratio calculation and classification rules

```
aspect_ratio = video_width / video_height     (intrinsic dimensions, when available)

Portrait:            aspect_ratio < 0.85
Square / near-square: 0.85 <= aspect_ratio <= 1.15
Landscape:            aspect_ratio > 1.15
```

Classification uses tolerant ranges (never exact equality). It drives layout class only; the
exact intrinsic ratio is always preserved when rendering (`media_frame_size` derives one
dimension from the other so the result is ratio-exact to floating precision). Enforced by
`aspect_ratio_class_uses_tolerant_spec_ranges` and the Task 19 matrix (ratio-exactness asserted
to 1e-4 wide / 1e-3 narrow).

## 5. Poster-generation approach

- Poster generated only after enough video data is available (after `DownloadDone` on the local
  side; on the receiver side after the bounded poster-blob fetch).
- Uses the existing ffmpeg toolchain with `-autorotate` (orientation from metadata), scale
  `min(320,iw):-2`, WebP q80 — colour and aspect preserved, never upscaled.
- Content-addressed cache `{blake3}.webp`: read-before-generate; invalidated by content-hash key.
- Runs off the UI thread (`Task::perform` + `spawn_blocking` at all call sites).
- File-type placeholder while pending; Err paths keep the placeholder (no fake poster).
- Bounds: `MAX_POSTER_EDGE=320`, `MAX_POSTER_BYTES=512KiB`, `MAX_POSTER_INPUT_BYTES=512MiB`,
  decoded-dimension bound `MAX_POSTER_DECODED_EDGE=1280` before the image decoder runs
  (decompression-bomb guard, VIDCARD-18).

## 6. Confirmations

- **`contain`-style rendering is used**: `iced::ContentFit::Contain` for both the poster and the
  player element (`video_file_card.rs:947`, `:1147`); a structural test asserts the card body
  contains `ContentFit::Contain` and no `Cover` for the primary preview.
- **Poster and player share identical geometry**: both render through the same
  `MediaFrameSizing` (width, height, exact ratio, border radius, media background, boundary
  clip, position). Playback replaces the poster in-frame via one media element — no card
  rebuild, no layout jump, no scroll loss (VIDCARD-10, structural test `9cfb9726`).

## 7. Confirmation that file-transfer and encryption logic were not changed

Verified commit-by-commit across all 21 VIDCARD commits on `origin/main`:

- **No protocol/payload changes**: `Message::FileShare` still carries `name/ticket/size/
  thumbnail_hash`. No serialization, wire, or blob-ticket format changed. The only
  `chat_core`/`system_events` change is the *display text* of the surrounding system line
  (`"shared a file"` without the repeated filename) — backward compatible with older persisted
  lines.
- **No transfer-engine changes**: `blob_transfer`, `download_manager`, `file_access_*`,
  `catalogue_*`, `download_initiation`, `download_limits` and every other transfer/access
  module are untouched.
- **No encryption/permission changes**: no crypto module touched; sender and content-address
  verification intact (verified in VIDCARD-18).
- **No playback-logic changes**: `src/video_playback.rs` change is purely additive
  (139 insertions / 0 deletions — a metadata probe); the player itself is unchanged.
- `src/video_poster.rs` changes are additive bounds/guardrail helpers.
- All remaining changes are confined to `examples/iced_chat/` (UI) and test targets.

## 8. Screenshots

**Gap (noted per close-out scope item 5):** no VIDCARD card produced screenshot evidence.
Only VIDCARD-01 attached an artifact (the architecture note). The app has no screenshot mode
and the project has no headless-capture harness; the cards verified the redesign through
structural tests and the Task 19/20 test matrices instead. No screenshots exist for 16:9,
square, 9:16, narrow-window, download-progress, or failed-state renders, and generating them
would require implementing a capture harness (out of scope for this review/report task).

The closest available *machine-verifiable* stand-ins are:
- VIDCARD-19 aspect-ratio matrix (below): exact rendered frame dimensions for every ratio at
  wide and narrow widths.
- Structural tests in `video_file_card.rs` (geometry, contain-style, centring, overlay,
  duration badge, actions accessibility) — see §9.

## 9. Results of the aspect-ratio test matrix (VIDCARD-19)

All 10 spec cases PASS at wide and narrow widths (commit `89216eb8`; 47/47 video_file_card
tests at final commit). No stretch/squash/crop, no excessive card height, no horizontal
overflow; play overlay centred; poster/player identical geometry.

| Case | Class | Wide frame | Narrow frame | Result |
|---|---|---|---|---|
| Standard landscape 1920×1080 | Landscape | 720×405 | 352×198 | PASS |
| HD landscape 1280×720 | Landscape | 720×405 | 352×198 | PASS |
| Ultrawide 2560×1080 | Landscape | 720×303.75 | 352×148.5 | PASS |
| Classic landscape 640×480 | Landscape | 666.67×500 | 352×264 | PASS |
| Square 1080×1080 | Square | 480×480 | 336×336 | PASS |
| Near-square 1080×1200 | Square | 468×520 | 327.6×364 | PASS |
| Vertical 1080×1920 | Portrait | 292.5×520 | 204.75×364 | PASS |
| Tall vertical 720×1600 | Portrait | 234×520 | 163.8×364 | PASS |
| Small landscape 320×180 | Landscape | 720×405 | 352×198 | PASS |
| Unknown metadata (no dims) | Landscape (16:9 fallback) | 720×405 | 352×198 | PASS |

## 10. Results of the functional test matrix (VIDCARD-20)

17/17 spec items pass (15 PASS as-is; 2 UI-level failures found and fixed — commit `3a82499f`):
Retry/Download buttons were silent no-ops for Cancelled/retryable-Failed/FileRemoved/missing-file
states (fixed via `download_restartable()` predicate), and late `DownloadDone` could flip a
user-cancelled card back to Ready (fixed via terminal-state guard). Item-level evidence:

- Video downloads successfully — PASS
- Progress updates correctly — PASS
- Cancel works where supported — PASS (message fixed: "Cancel requested.")
- Retry works where supported — FIXED (was no-op)
- Play starts the correct video — PASS
- Pause and seek work — PASS
- Opening the file works — PASS (safe OS open)
- Opening the downloads folder works — PASS
- Re-share preserves existing behaviour — PASS (same FileShare path as before redesign)
- The overflow menu works — PASS
- Deleted local files show a useful state — PASS (button fixed)
- Failed thumbnails show a useful fallback — PASS
- Failed decoding does not crash the chat — PASS
- Reloading the conversation restores the correct card state — PASS
- Multiple video cards can exist in the same chat — PASS
- A playing video does not interfere with another card's layout — PASS
- Incoming and outgoing video cards remain visually consistent — PASS

## 11. Acceptance-criteria verification (spec §Acceptance Criteria, 17 items)

1. Card matches approved Boru visual style — PASS (VIDCARD-03 + structural test)
2. Landscape/square/portrait display correctly — PASS (VIDCARD-19 matrix)
3. Intrinsic aspect ratios preserved — PASS (ratio-exactness asserted 1e-4/1e-3)
4. No stretch/squash/unintentional crop — PASS (`Contain` everywhere, matrix)
5. Portrait uses narrower centred preview — PASS (VIDCARD-08, matrix)
6. Square uses centred width-capped preview — PASS (VIDCARD-07, matrix)
7. Landscape uses horizontal space efficiently — PASS (VIDCARD-06, 720×405 typical)
8. Poster/player geometry identical — PASS (VIDCARD-10 structural test)
9. Unknown dimensions handled safely — PASS (VIDCARD-09, 16:9 fallback + bounded frame)
10. Filenames cannot widen/break the card — PASS (VIDCARD-04 truncation + tooltip)
11. Metadata and actions clearly organised — PASS (VIDCARD-12/13)
12. Old blue action-button styling removed — PASS (VIDCARD-13)
13. Download progress clear and accessible — PASS (VIDCARD-14/17)
14. Card works at wide/medium/narrow widths — PASS (VIDCARD-15; matrix at wide+narrow)
15. Transfer/playback/encryption/re-share logic unchanged — PASS (see §7)
16. All aspect-ratio and functional test cases pass — PASS (19: 10/10; 20: 17/17)
17. Screenshots supplied for 16:9/square/9:16 — **NOT MET** (see §8; no card produced
   screenshots; capture harness out of scope)

## 12. Board / git close-out verification

- All 20 VIDCARD cards (01–20) are **done**; no card remains blocked/todo.
- `git fetch origin`; all 21 VIDCARD commits are ancestors of `origin/main`
  (`git merge-base --is-ancestor` clean for every commit); `HEAD == origin/main == 99ac0a04`
  (VIDCARD-17 commit, latest). Nothing lost.
- Worktree `wt/t_78f5b5d2` HEAD == canonical `/home/dan/iroh-gossip-chat` HEAD.
- Compilation verified on debsrv: `rb check --example boru --features gui,video-playback,terminal`
  from `/home/dan/iroh-gossip-chat` → exit 0, "Finished dev profile in 7.84s".
- Test evidence at final commits: video_file_card 47/47 (VIDCARD-19), download_restartable +
  download_lifecycle tests (VIDCARD-20), 73 targeted accessibility tests (VIDCARD-17), full
  example suites 898–982 green across intermediate commits (VIDCARD-02..17).

## 13. Follow-up recommendation

Create a dedicated screenshot-capture task if visual evidence is required (e.g. an offscreen
render harness or a live two-peer capture session under Xvfb). This close-out task's scope was
review/verify/report only, so the capture harness was not implemented here.
