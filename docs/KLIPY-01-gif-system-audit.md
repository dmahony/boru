# KLIPY-01: GIF System Architecture Audit

**Task:** KLIPY-01 — Inspect the existing GIF implementation (baseline evidence for the KLIPY GIF Search Provider Abstraction epic)
**Repo:** iroh-gossip-chat (Rust, Boru iced app)
**Date:** 2026-08-08
**Status:** Analysis only — no production code modified.

This note documents Boru's current external GIF implementation before any KLIPY work begins.
All references are to the canonical repo (examples/iced_chat/app.rs is ~50k lines; line numbers verified against the worktree at commit `dcf7430b`).

---

## 1. Current GIF search provider: GIPHY

The only external GIF provider is **GIPHY**, integrated inline in the iced app state machine (`examples/iced_chat/app.rs`). There is no provider abstraction, no separate module, and no provider-neutral domain model — everything lives in the app's `update()` handler and view code.

### 1.1 API key

- **Hardcoded API key** const inside the `GifSearchSubmit` handler: `app.rs:21575`
  ```rust
  const API_KEY: &str = "[REDACTED — removed in KLIPY-08]";
  ```
- There is **no environment variable, config file entry, or feature flag** for the key (verified: no `KLIPY_API_KEY`, `GIPHY_API_KEY`, or any GIF-related env var anywhere in the repo; `docs/configuration.md` lists only `BORU_*` vars).
- The key ships in the binary and is sent to `api.giphy.com` on every search.

### 1.2 Search endpoint and request construction

- GIPHY search endpoint: `app.rs:21594`
  ```
  https://api.giphy.com/v1/gifs/search?api_key={}&q={}&limit=20&rating=g&lang=en
  ```
- Query is URL-encoded by a hand-rolled per-character encoder: `app.rs:21576-21592` (spaces → `%20`, alphanumerics kept, everything else `%XX`).
- Hardcoded parameters: `limit=20`, `rating=g`, `lang=en`. **No pagination/cursor**, **no trending call**, **no content-rating selection UI**.
- Errors are swallowed: `resp.send().await.ok()?` and `resp.json().await.ok()?` (`app.rs:21598-21599`); a failure produces `None` → `AppMessage::Noop` with a single `tracing::warn!("GIPHY search failed or returned no results")` at `app.rs:21628`.

### 1.3 HTTP client

- Two ad-hoc `reqwest::Client::new()` instances are created per operation — one for the search request (`app.rs:21597`), one for the send/download request (`app.rs:21649`). No shared client, no timeouts, no retries.
- `reqwest` is an **optional dependency** with `default-features = false`, `features = ["rustls-tls", "json"]` — `Cargo.toml:135` — enabled only by the `gui` feature — `Cargo.toml:216`.

---

## 2. GIF search result model

- `struct GifResult` at `app.rs:2968-2975`:
  ```rust
  struct GifResult {
      title: String,          // gif["title"]
      full_url: String,       // gif["images"]["original"]["url"]
      preview_bytes: Vec<u8>, // downloaded GIF bytes for the thumbnail
  }
  ```
- Response parsing is done directly against `serde_json::Value` (`app.rs:21601-21622`): iterates `body["data"]`, reads `title`, `images.original.url`, `images.fixed_height.url`, then **downloads the preview GIF bytes inline** (`app.rs:21607-21616`) and stores them in `preview_bytes` for the picker thumbnail.

---

## 3. App state (GIF-specific)

Fields on the main app struct (`app.rs:4093-4098`):

| Field | Type | Purpose | Init |
|---|---|---|---|
| `show_gif_picker` | `bool` | Picker overlay visibility | `app.rs:7451` (`false`) |
| `gif_search_text` | `String` | Picker search input | `app.rs:7452` (default) |
| `gif_results` | `Vec<GifResult>` | Search results with preview bytes | `app.rs:7453` (`Vec::new()`) |

There is **no GIF-specific message persistence, DB table, or store** — see §7.

---

## 4. Messages / event loop

GIF-related `AppMessage` variants (`app.rs:5888-5897`):

| Variant | Payload | Purpose | Handler |
|---|---|---|---|
| `ToggleGifPicker` | — | Open/close picker overlay | `app.rs:21557-21560` |
| `GifSearchChanged(String)` | query text | Update `gif_search_text` (no debounce — text is sent on every keystroke) | `app.rs:21562-21565` |
| `GifSearchSubmit` | — | Fire the GIPHY search task | `app.rs:21567-21634` |
| `GifSearchResults(Vec<GifResult>)` | results | Store results into `gif_results` | `app.rs:21636-21639` |
| `SendGifUrl(String)` | full URL | Download asset → temp file → `ExecuteImageSend` | `app.rs:21641-21670` |

Debug/name mapping: `app.rs:9461-9465`.

### 4.1 Send flow (picker → chat)

`SendGifUrl` (`app.rs:21641-21670`):
1. Closes the picker and clears search text (`app.rs:21643-21644`).
2. Shows toast `"Downloading GIF..."` (`app.rs:21645-21646`).
3. Downloads the full-size GIF from `full_url` via a fresh `reqwest::Client` (`app.rs:21649-21655`).
4. Writes bytes to `<temp_dir>/boru_gif_<pid>.gif` (`app.rs:21656-21657`).
5. Encodes as `"gif.gif|<abs_path>|"` and dispatches `AppMessage::ExecuteImageSend(encoded)` (`app.rs:21662-21665`), i.e. the selected GIF goes through the **standard encrypted image-attachment pipeline** (see §8).

---

## 5. GIF picker UI

- **Overlay wiring:** on the chat screen, when `show_gif_picker` is true the picker is layered over the chat panel with `iced::widget::Stack`, bottom-right aligned, 48px bottom padding (`app.rs:28803-28823`). Same pattern as the emoji picker overlay.
- **Composer toggle:** a text button labelled `"GIF"` in the composer trailing actions (`app.rs:31807-31814`) dispatches `ToggleGifPicker`.
- **Picker view:** `view_gif_picker()` at `app.rs:28963-29095`:
  - Header: `"GIF Search"` card title + close button (`app.rs:28976-28983`).
  - Search row: `text_input` with placeholder `"Search GIFs…"` bound to `GifSearchChanged`/`GifSearchSubmit`, plus a `"Search"` button enabled when text is non-empty (`app.rs:28985-29004`).
  - Results: empty state text `"Type a search term and press Enter or Search"` when `gif_results.is_empty()` (`app.rs:29008-29015`); otherwise a 2-column grid of 150×100 thumbnails rendered from `preview_bytes` via `iced::widget::image::Handle::from_bytes` (`app.rs:29017-29074`), each a button dispatching `SendGifUrl(full_url)` (`app.rs:29067`).
  - Scroll area 300px high (`app.rs:29077`); panel width 320px (`app.rs:29093`).
- **Missing states:** no loading indicator, no no-results-after-search state, no network-error state, no provider-not-configured state, no trending/suggested section, no pagination, no keyboard navigation, no stale-response guard (an older `GifSearchResults` can overwrite newer results if requests complete out of order — there is no request-id/sequence check).

---

## 6. GIF rendering inside chat messages

- `ChatEntry` carries `gif_frames: Option<Arc<iced_moving_picture::widget::gif::Frames>>` (`app.rs:2465`), populated by `ChatEntry::image()` via `decode_gif_frames(&image_bytes)` (`app.rs:2663`).
- `decode_gif_frames()` (`app.rs:2498-2520`): counts frames with `image::codecs::gif::GifDecoder`; single-frame GIFs return `None` (render as static images); multi-frame GIFs are decoded via `iced_moving_picture::widget::gif::Frames::from_bytes`.
- **Inline chat rendering:** animated GIFs render with `iced_moving_picture::widget::gif::Gif::new(frames)` scaled to fit, in a bordered card (`app.rs:31517-31544`); static images use the cached image handle (`app.rs:31545+`).
- **Lightbox:** the full-screen image lightbox also renders animated GIFs via the same `Gif` widget (`app.rs:24644-24659`).
- Dependency: `iced_moving_picture = { version = "0.2", default-features = false, features = ["tokio"], optional = true }` — `Cargo.toml:129`, behind `gui` (`Cargo.toml:216`).
- Receiver-side image decode happens once at `ChatEntry::image` construction (`app.rs:2628-2678`); dimensions extracted for scale-to-fit (`app.rs:2639-2648`).

---

## 7. Message types / DB fields / persistence

- **There is no GIF-specific message variant.** GIFs are sent as generic `Message::ImageShare { name, hash }` (`src/chat_core.rs:966-971`) — name + blake3 blob hash, no provider, no URL, no tracking fields.
- **No GIF-specific database fields.** `HistoryEntry` (`src/chat_history.rs:169-213`) stores image data as:
  - `image_bytes: Option<Vec<u8>>` — `#[serde(skip)]`, in-memory only (`src/chat_history.rs:195-197`)
  - `image_identifier: Option<String>` — persisted; `<user-hash>/<content-hash>.<ext>` relative path in the ImageStore (`src/chat_history.rs:198-204`)
- **Persistence path for image/GIF messages:** on `ImageDownloaded` the app writes a `HistoryEntry` with `kind="image"`, `image_bytes` (session-only) and `image_identifier` (durable) (`app.rs:17319-17337`).
- `ImageStore::save_image()` (`src/image_store.rs:26-86`): content-addressed storage under `<files_root>/<user-hash>/<content-hash>.<ext>`; extension allow-list `png|jpg|jpeg|gif|webp|bmp`, everything else `.bin` (`src/image_store.rs:50-54`). Test coverage for `.gif` at `src/image_store.rs:356`.

**Conclusion:** External GIF content is **not persisted as a dedicated GIF message**. The full asset is downloaded to a temp file, re-uploaded through the standard `ImageShare` encrypted-attachment pipeline, and stored like any other image (bytes in-memory during the session, ImageStore identifier on disk). The only GIF-specific traces are the file extension and the `decode_gif_frames` rendering path.

---

## 8. User-uploaded GIF handling (attachment + encrypted file-transfer)

User-selected animation files share the **generic attachment pipeline** — they never touch GIPHY/KLIPY.

1. **OS file picker:** `AttachPressed` (`app.rs:13984-14011`) opens `rfd::AsyncFileDialog`, detects image extensions (`.png/.jpg/.jpeg/.gif/.webp/.bmp`, `app.rs:13995-14000`) and dispatches `ExecuteImageSend(encoded)`; drag-and-drop uses the same detection (`app.rs:14058-14082`). Non-image files go to `ExecuteFileSend` (FileShare path).
2. **`ExecuteImageSend` pipeline** (`app.rs:15790-15919`):
   - Size validation against `CHAT_IMAGE_MAX_BYTES = 10 MiB` (`app.rs:15820`, const at `src/image_optimizer.rs:27`).
   - **GIF special-case:** `.gif` files are transmitted **byte-for-byte unchanged** with MIME `image/gif` to preserve animation frames — no WebP conversion (`app.rs:15832-15841`). All other image types are converted to lossless WebP via `optimize_chat_image_to_webp` (`app.rs:15848-15850`, `src/image_optimizer.rs:305`).
   - Bytes are added to the iroh blob store (`blob_store.blobs().add_bytes`, `app.rs:15873-15877`), registered as a chat upload (`register_chat_upload`, `app.rs:15882-15889`), signed into `Message::ImageShare { name, hash }` (`app.rs:15891-15896`), and broadcast over gossip (`app.rs:15897-15898`).
3. **Receiver side:** `Message::ImageShare` → `set_pending_image` (`src/chat_core.rs:2036-2043`, `src/chat_core.rs:778-779`) → `start_next_pending_image_download` (`app.rs:8135-8200`): fetches the blob via `download_blob_with_safety` (iroh blobs over QUIC with candidate selection), skips thumbnail compression for `.gif` (`app.rs:8172-8177`), saves to the per-user ImageStore (`app.rs:8181`), and completes with `ImageDownloaded` → `ChatEntry::image` + history persistence (`app.rs:17243-17338`).
4. **MIME mapping** for files/attachments: `src/file_indexer.rs:706` (`"gif" => "image/gif"`); wire-compression extension/mime tables mention gif at `src/wire_compression.rs:1322,1580-1581`.
5. **Non-image animation files** (MP4, animated WebP selected as files rather than images) travel the generic `Message::FileShare { name, ticket, size, ... }` path (`src/chat_core.rs:899-926`) — same encryption, transfer, permissions, progress, retry, and storage as any other file.

**Security/authorisation:** file sharing goes through the existing permission model (friends/permission checks + `download_blob_with_safety`), and the GIPHY send path re-uses `ExecuteImageSend`, so a provider-selected GIF is treated exactly like a locally-attached GIF. Nothing bypasses attachment permissions.

---

## 9. Analytics / attribution / tracking / caching / proxying

- **None.** No GIPHY attribution, "Powered by GIPHY" branding, analytics, tracking parameters, caching layer, or proxy for the GIF provider (verified by repo-wide search for `giphy`, `attribution`, `analytics`, `tracking`, `proxy`).
- The GIPHY response fields are not stored anywhere except `GifResult` (title/full_url/preview bytes), and the sent message is only `ImageShare{name, hash}` — no provider metadata reaches peers or history.

---

## 10. Tests and documentation related to the old GIF system

- **No tests exercise the GIPHY search/picker/send flow.** (No references to `GifSearch`, `GifResult`, `SendGifUrl`, or `ToggleGifPicker` outside `app.rs`.)
- GIF-related test coverage that exists:
  - `tests/image_optimizer_integration.rs:215-237` — single-frame GIF accepted + animated GIF first-frame decoded by `optimize_chat_image` (fixtures generated by `tests/generate_test_images.py:99-112`).
  - `src/image_store.rs:356` — ImageStore saves a `.gif` file.
  - Generic ImageShare E2E tests (`tests/test_image_send_download.rs`, `test_image_receiver_download.rs`, `test_image_iced_gui_flow.rs`, `test_multi_image_burst.rs`, `test_image_cache_persistence.rs`) cover the image attachment pipeline that GIFs reuse, but none use `.gif` fixtures.
- Documentation mentions:
  - `docs/video-inline-playback/step1-baseline.md:136-138` — describes `decode_gif_frames` animated-GIF rendering.
  - `docs/message-storage-design.md:450` — files-dir extension allow-list incl. `gif`.
  - `docs/ui-redesign/current-ui-map.md:236` — "Emoji/GIF controls → toggle picker, search, insert/send, advance animation".
  - `docs/ui-redesign/evidence/ui-15/README-composer.md:48` and `docs/ui-redesign/evidence/ui-17/interaction-checklist.md:41` — GIF button/picker toggle references.
  - `docs/file-type-icons/PAPIRUS-01-file-sharing-surfaces.md:161-163` — MIME/extension maps incl. gif/webp.
  - `docs/fonts/FONTS-01-typography-audit.md:280` — `view_gif_picker()` font role.
  - `docs/ui-redesign/evidence/ui-17/regression-matrix.md:28` — Emoji/GIF wiring.
- **No developer documentation** tells users how to configure GIPHY (there is nothing to configure — the key is compiled in).

---

## 11. What must be replaced (KLIPY)

### Remove / replace (GIPHY integration)

| Site | Current | KLIPY replacement |
|---|---|---|
| `app.rs:21575` | Hardcoded GIPHY API key | Config/env (`KLIPY_API_KEY`), `SecretString`, no-key graceful disable |
| `app.rs:21572-21634` | `GifSearchSubmit` inline GIPHY search | `GifProvider::search()` via generic trait + `KlipyGifProvider` adapter |
| `app.rs:21641-21670` | `SendGifUrl` downloads original + temp file | Provider-neutral `SharedGif` payload (playback/preview URLs) OR retain attachment pipeline per spec; remove full-size download where preview/playback renditions exist |
| `app.rs:21601-21622` | Direct `serde_json::Value` parsing of GIPHY fields | KLIPY-specific models confined to the adapter module |
| `app.rs:2968-2975` | `GifResult { title, full_url, preview_bytes }` | Provider-neutral `GifSearchResult` / `GifMediaSource` models |
| `app.rs:28963-29095` | Picker UI (search only, no states) | Empty / search / trending / loading / no-results / error / not-configured states, pagination, stale-response rejection |
| `app.rs:21597,21649` | Ad-hoc `reqwest::Client::new()` | Shared client in provider adapter (timeouts, cancellation, rate-limit handling) |
| `app.rs:5888-5897` | GIPHY-coupled message variants | Keep picker messages but drive them through `GifProvider`; add provider-not-configured handling |

### Preserve (out of scope for replacement)

- `Message::ImageShare` + `ExecuteImageSend` attachment pipeline (`app.rs:15790-15919`) — provider-selected GIFs and user-uploaded GIFs both flow here; must remain the encrypted attachment path for uploaded files (KLIPY-07).
- `decode_gif_frames` + `iced_moving_picture` rendering (`app.rs:2498-2520`, `app.rs:31517-31544`, `app.rs:24644-24659`) — handles GIF playback inside chat.
- `ImageStore` persistence (`src/image_store.rs`) and `HistoryEntry` image fields (`src/chat_history.rs:195-204`).
- Generic `FileShare` path for MP4/WebP-as-file attachments (`src/chat_core.rs:899-926`).

### Notes for downstream KLIPY cards

- The picker is overlay-wired the same way as the emoji picker (Stack overlay, `app.rs:28803-28823`) — KLIPY UI work should reuse this pattern.
- `reqwest` already exists behind `gui` (`Cargo.toml:135,216`) — KLIPY can reuse it without adding a second networking stack.
- Message serialization is postcard-based (`SignedMessage` envelope, `src/chat_core.rs:1018-1236`); any new `SharedGif` payload must follow the existing serde/postcard conventions with tolerant deserialization for backward compatibility (see `deserialize_tolerant_*` helpers at `src/chat_core.rs:1032+`).
- No analytics/attribution currently exists; KLIPY privacy requirements (no peer IDs/usernames/room IDs in search, no query logging, no invisible tracking params) are greenfield.
- GIPHY remnants to sweep after KLIPY lands (Step 8 of the spec): the only GIPHY references are `app.rs:5894` (doc comment), `app.rs:21574` (comment), `app.rs:21575` (key), `app.rs:21594` (URL), `app.rs:21628` (warn log). No env vars, assets, feature flags, or docs reference GIPHY.

---

## 12. Baseline verification

- Analysis performed in worktree `wt/t_902c3eab` at `dcf7430b` (TUN-UI: cap Create Tunnel share dialog body so footer stays visible).
- No production code modified (`git status` clean except this note).
- Build check: `rb check --example boru --features gui,video-playback,terminal` run against the canonical repo (see task record for result).
