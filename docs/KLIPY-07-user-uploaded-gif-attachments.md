# KLIPY-07: Preserve user-uploaded GIF attachments

**Task:** KLIPY-07 — confirm and preserve Boru's user-uploaded animation-file handling
**Repo:** iroh-gossip-chat (Rust, Boru iced app)
**Date:** 2026-08-08
**Status:** Analysis + regression tests; no change to the attachment pipeline itself.

User-uploaded animation files (`.gif`, animated `.webp`, `.mp4` and similar) are **ordinary
Boru attachments**. They are separate from external GIF search results and must never be
uploaded to KLIPY or converted into provider-GIF messages. This note documents the full path
and the regression tests that lock it in.

All line references verified against the worktree at commit `867a0af0` (KLIPY-01 audit landed).

---

## 1. The complete user-uploaded animation-file path

### 1.1 OS file picker / drag-and-drop entry

| Entry point | Location | Behaviour |
|---|---|---|
| Attach button (OS file picker) | `AppMessage::AttachPressed` handler, `examples/iced_chat/app.rs:14028-14055` | Opens `rfd::AsyncFileDialog::pick_file()`. Extension detection at `app.rs:14039-14044`: `.png/.jpg/.jpeg/.gif/.webp/.bmp` → `ExecuteImageSend` (image attachment pipeline); everything else → `ExecuteFileSend` (generic file pipeline). |
| Drag-and-drop into composer | `AppMessage::ComposerFileDropped` handler, `app.rs:14102-14126` | Same extension rule at `app.rs:14115-14120`: `.gif/.webp/.bmp` → `ExecuteImageSend`; `.mp4`/`.mov`/others → `ExecuteFileSend`. |

The encoded payload for both is `"{name}|{abs_path}|{abs_path}"`.

**Key guarantee:** `.gif` and animated `.webp` selected by the user are routed to the
**image attachment pipeline**, not to the GIF search provider. The provider path
(`GifSearchSubmit` / `SendGifUrl`, `app.rs:21567-21670`) is only reachable from the
picker overlay (the `"GIF"` composer button), never from the file picker or drop handler.

### 1.2 `ExecuteImageSend` — send-side attachment pipeline

`AppMessage::ExecuteImageSend` handler, `app.rs:15834-15963`:

1. **Size validation** (`app.rs:15861-15869`): rejects files > `CHAT_IMAGE_MAX_BYTES`
   (10 MiB, `src/image_optimizer.rs:27`) before reading.
2. **GIF special case** (`app.rs:15876-15885`): when the filename ends in `.gif`,
   bytes are transmitted **byte-for-byte unchanged** with MIME `image/gif` — no WebP
   conversion, so animation frames survive end-to-end. This is the "do not duplicate the
   attachment implementation for GIF" guarantee: GIFs reuse the identical `ExecuteImageSend`
   pipeline as PNG/JPEG/WebP, with only the re-encode step skipped.
3. **Non-GIF images** (`app.rs:15886-15912`): converted to lossless WebP via
   `optimize_chat_image_to_webp` (`src/image_optimizer.rs:305`). A user-selected animated
   WebP is flattened to a static first-frame WebP here (existing behaviour, unchanged).
4. **Content-addressed transfer** (`app.rs:15917-15924`): bytes added to the iroh blob store
   via `blob_store.blobs().add_bytes()`, producing a BLAKE3 `MessageHash`.
5. **Attachment metadata/authorisation** (`app.rs:15925-15934`): `register_chat_upload`
   records the upload in the local storage (MIME `image/gif` preserved for GIFs).
6. **Encrypted/signed message** (`app.rs:15935-15943`): constructs
   `Message::ImageShare { name, hash }` (`src/chat_core.rs:966-971`) and broadcasts it as a
   signed gossip envelope (`SignedMessage::sign_and_encode`).

No provider field exists on `ImageShare` — only `name` and `hash`. There is no wire path
from this handler to GIPHY or KLIPY.

### 1.3 Receiver-side download

1. `Message::ImageShare` → `set_pending_image` (`src/chat_core.rs:2036-2043`, friendship
   check at `chat_core.rs:2039`: `cb.is_friend(&from) || cb.accepts_group_peer(...)`).
2. `start_next_pending_image_download` (`app.rs:8179-8244`) calls
   `download_blob_with_safety` (`src/chat_core.rs:2792`) — the same authorisation/safety
   path used by every image download:
   - `PublicRoomSafety::try_acquire_download` per-peer queue admission when in a public
     room (`src/public_room_safety.rs:244`),
   - `max_blob_size_bytes` enforcement (`src/public_room_safety.rs:106`).
3. **GIF skip-compression** (`app.rs:8216-8221`): `.gif` downloads skip JPEG thumbnail
   re-encoding (`thumb = buf.clone()`), so `decode_gif_frames` on the receiver can extract
   the original animation frames. Other formats get a lightweight display thumbnail.
4. **Storage** (`app.rs:8224-8225`): `image_store.save_image(&user, &name, &thumb)`
   (`src/image_store.rs:55`) — content-addressed under `<files_root>/<user-hash>/<content-hash>.<ext>`;
   the extension allow-list preserves `gif` (`src/image_store.rs:50-54`).
5. `ImageDownloaded` (`app.rs:17243-17338`): creates the chat entry with
   `ChatEntry::image` (`app.rs:2628-2678`) and persists a `HistoryEntry` with
   `image_identifier`.

### 1.4 Generic file path for non-image animation files (MP4, MOV, MKV, WEBM, animated WebP-as-file)

When the user selects a file that is *not* in the image extension list (notably `.mp4`,
`.mov`, `.mkv`, `.webm`), `AttachPressed`/`ComposerFileDropped` route to
`ExecuteFileSend` → `Message::FileShare { name, ticket, size, thumbnail_hash, ... }`
(`src/chat_core.rs:899-926`). The receiver stores a download card and downloads via
`download_blob_to_file` (`src/chat_core.rs:2661`) with:

- transfer progress events (`TransferProgress::Started/Progress/Completed`,
  `src/chat_callbacks.rs:78-130`),
- cancel guard / retry via `download_restartable` + `ExecuteDownloadAt`
  (`app.rs:15975+`),
- optional video poster thumbnail (`thumbnail_hash`), rendered by `BoruVideoFileCard`
  (`examples/iced_chat/video_file_card.rs`).

This path is identical for MP4 and for any other file — no animation-specific code.

---

## 2. Renderers after download

| Format | Upload path | Receiver renderer | Notes |
|---|---|---|---|
| `.gif` (animated) | `ExecuteImageSend` byte-for-byte | `decode_gif_frames` (`app.rs:2498-2520`) → `iced_moving_picture::widget::gif::Frames` → `Gif` widget in chat (`app.rs:31517-31544`) and lightbox (`app.rs:24644-24659`) | Multi-frame GIFs animate; single-frame GIFs render as static images. |
| `.gif` (static) | `ExecuteImageSend` byte-for-byte | static image path (`decode_gif_frames` returns `None`, `app.rs:2511-2513`) | Existing behaviour. |
| `.webp` (animated) | `ExecuteImageSend` → optimizer flattens to static first-frame WebP | `iced::widget::image` handle | Animation is not preserved for WebP in the image attachment path (existing behaviour, unchanged by KLIPY-07). The `image` crate's `WebPDecoder` also implements `AnimationDecoder` (`image-0.25.10/src/codecs/webp/decoder.rs:104`), but the app only animates GIFs today. |
| `.webp` (static) | `ExecuteImageSend` → lossless WebP | `iced::widget::image` handle | Renders inline. |
| `.mp4` / video | `ExecuteFileSend` → `FileShare` | `BoruVideoFileCard` + `iced_video_player::VideoPlayer` (video-playback feature) | Inline playback after download; poster thumbnail shown while downloading. |

---

## 3. KLIPY separation (no-code-change guarantees)

- `Message::ImageShare` has exactly two fields (`name`, `hash`) — no provider, no URL, no
  KLIPY/GIPHY metadata (`src/chat_core.rs:966-971`).
- The attachment entry points (`AttachPressed`, `ComposerFileDropped`) route to
  `ExecuteImageSend`/`ExecuteFileSend` and never construct `GifSearchSubmit`/`SendGifUrl`.
- The provider search/send flow is only wired to the picker overlay button (`app.rs:31807-31814`).
- No code path reads the provider API key or constructs a provider request from a
  user-selected file.

---

## 4. Regression tests

### 4.1 New integration test file: `tests/test_user_uploaded_gif.rs` (feature `net`)

| Test | Confirms |
|---|---|
| `gif_attachment_roundtrip_preserves_bytes` | A real animated GIF sent via `ImageShare` between two localhost peers downloads byte-for-byte identical (encrypted attachment pipeline, no conversion). |
| `png_attachment_roundtrip_preserves_bytes` | A PNG uses the same pipeline and round-trips unchanged (other file types unaffected). |
| `image_share_wire_message_has_no_provider_fields` | Serialized `Message::ImageShare` contains only `name`/`hash` — no provider/URL/KLIPY fields. |
| `mp4_file_share_progress_emits_started_and_completed` | A `.mp4`-named `FileShare` download emits `TransferProgress::Started` and `Completed` (progress/retry path still works). |
| `gif_download_permissions_enforced_by_safety_size_cap` | `download_blob_with_safety` with a `PublicRoomSafety` size cap rejects an oversized GIF (attachment permissions remain enforced). |
| `image_store_saves_gif_with_gif_extension` | `ImageStore.save_image` stores a `.gif` under the `gif` extension (storage behaviour unchanged). |

### 4.2 app.rs unit-test additions (in the existing `#[cfg(test)] mod tests`)

| Test | Confirms |
|---|---|
| `attachment_image_detection_covers_gif_webp_bmp` | The shared `is_attachment_image` routing helper returns `true` for `.gif`/`.webp`/`.bmp` (user-selected animation images still use the image attachment pipeline). |
| `attachment_image_detection_excludes_video_and_text` | Returns `false` for `.mp4`/`.mov`/`.txt` (video/other files still use the file pipeline). |
| `decode_gif_frames_animated_returns_some` | `decode_gif_frames` yields `Frames` for a real multi-frame GIF (GIF renderer works after download). |
| `decode_gif_frames_single_frame_returns_none` | Single-frame GIF stays on the static image path. |
| `decode_gif_frames_non_gif_returns_none` | Non-GIF bytes (PNG) do not enter the animated path. |

### 4.3 Build/test

- `rb check --bin boru --features gui,video-playback,terminal` (remote debsrv build).
- Targeted `rb test --test test_user_uploaded_gif -- gif` (integration).
- Targeted `rb test --bin boru --features gui,video-playback,terminal -- decode_gif_frames` (renderer unit tests).

---

## 5. Out of scope / deliberately unchanged

- The `ExecuteImageSend` GIF byte-for-byte branch (`app.rs:15876-15885`) — preserved.
- The `start_next_pending_image_download` GIF skip-compression branch (`app.rs:8216-8221`) — preserved.
- Animated WebP is flattened to a static first frame by the sender optimizer (existing
  behaviour; adding WebP animation support is not part of KLIPY-07).
- Provider search/send flow (KLIPY-03/06) — separate tasks.
- No production attachment code was modified by this task.
