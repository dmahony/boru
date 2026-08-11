# VIDCARD-01 — Existing Boru Video Download Card: Architecture Note

Task: `t_58b2cda9` — Inspect the existing Boru video download card.
Source spec: `Boru_video_download_card_redesign.txt` Task 1 (attachment of `t_6e936d77`).
Scope: analysis only. No production code was modified by this task.
Baseline: `cargo build --bin boru --features gui,video-playback,terminal` — see "Baseline evidence" at the end.

---

## 1. Component map (file:line)

| Concern | Location |
|---|---|
| Boru example entry | `examples/iced_chat/main.rs`; registered in `Cargo.toml:227-235` (`[[example]] name = "boru", required-features = ["gui"]`; `gui` feature at `Cargo.toml:208`, `video-playback = ["gui", "dep:iced_video_player"]` at `:209`, `terminal = ["gui", "dep:iced_term"]` at `:210`) |
| App state + messages + chat rendering | `examples/iced_chat/app.rs` (43,565 lines) |
| Download/video card renderer (stateless) | `examples/iced_chat/download_progress_view.rs` (980 lines) |
| Design system | `examples/iced_chat/design_tokens.rs`, `fonts.rs`, `card_shell.rs`, `icon_system.rs`, `ui_components.rs` |
| Poster generation (ffmpeg → bounded WebP) | `src/video_poster.rs` (module declared `src/lib.rs:64`) |
| Wire decode / net-event dispatch | `src/chat_core.rs` (`Message::FileShare` handling at `src/chat_core.rs:1981-1998`) |
| Incoming shared-file entry creation | `IcedChat::set_pending_file` — `app.rs:21433-21461` |
| Outgoing shared-file entry creation | `AppMessage::ExecuteFileSend` — `app.rs:14556-14700` |
| Download card embedded in chat log | `view_chat_log` — `app.rs:28189`; System download card branch `app.rs:28770-28804` |
| Per-card lazy render | `view_download_attachment` — `app.rs:7938-7978`; `view_download_attachment_content` `app.rs:7980-7997` |
| Card render internals | `view_download_progress` / `view_download_progress_with_player` / `view_download_progress_inner` — `download_progress_view.rs:237-737` |
| Inline player state/lifecycle | `InlineVideoSession` `app.rs:137-149`; `VideoInstanceKey` (near `app.rs:130-169`); viewport reconcile `app.rs:8807-8908`; `view_expanded_inline_video` `app.rs:22116-22193` |
| Dashboard "Downloaded" table | `view_downloads_card` — `app.rs:33068+`; row model `DownloadedItem` — `examples/iced_chat/downloaded_view_model.rs:79-100` |
| Prior architecture notes | `docs/video-inline-playback/step2-architecture.md`, `docs/design/download-progress-row.md`, `docs/inline-video-*.md` |

---

## 2. Chat message rendering path for shared video files

### Incoming (remote peer shared a video)
1. Gossip decode: `src/chat_core.rs:1981-1998` — `Message::FileShare { name, ticket, size, thumbnail_hash }` is verified and, for non-self senders, calls `cb.push_system(format!("{} shared a file: {}", sender_name, name))` then `cb.set_pending_file(name, ticket, size, thumbnail_hash)`.
2. `IcedChat::set_pending_file` (`app.rs:21433-21461`):
   - classifies video by extension via `classify_attachment(None, &name) == MediaKind::Video` → `TransferKind::Video` (`app.rs:21442-21446`);
   - creates `ChatEntry::system_download(..., xfer_kind, name, ticket, "", None)` — note `source_peer` is passed as `""` (`app.rs:21447-21454`), so incoming cards have no "From:" line;
   - sets `DownloadState::Ready { total: Some(size) }` and stores `thumbnail_hash` (`app.rs:21456-21459`). The poster blob is **not fetched at this point** — see gap G4.
3. Download completion: `AppMessage::DownloadDone` / `DownloadDonePeerFile` (`app.rs:15019-15082` / `15083-15133`) move the card to `Completed { saved_path: Some(path), ... }`; for video they run `video_poster::generate` in `spawn_blocking` (`app.rs:15064-15080`, `15116-15131`) and emit `AppMessage::PosterGenerated { name, poster }`.
4. `AppMessage::PosterGenerated` (`app.rs:15135-15155`): sets `download.poster_dimensions = dimensions` and `download.thumbnail = Some(bytes)` and clears layout cache — this is the first time an incoming card gets a real preview + dimensions (only after the local file download finishes).

### Outgoing (local user shared a video)
1. `AppMessage::ExecuteFileSend` (`app.rs:14556+`): `is_video = ChatEntry::is_video_file(&filename)` (`app.rs:14575`, helper at `app.rs:2455-2458` → `classify_attachment`); immediately pushes an upload card `ChatEntry::system_download` with `TransferKind::Video`, `state = Active { bytes: 0, total: Some(file_size) }`, `entry.body = "Uploading: {filename}"` (`app.rs:14586-14604`).
2. Upload task (`app.rs:14616+`): streams the file into iroh blobs, builds `BlobTicket` (`blob_ticket_string`, `app.rs:14636-14637`); for video, `video_poster::generate` in `spawn_blocking` → `thumbnail_bytes` (`app.rs:14641-14654`); stores the poster as a blob and puts `thumbnail_hash` into `Message::FileShare` (`app.rs:14657-14670`); signs + broadcasts.
3. `AppMessage::FileDownloaded` (`app.rs:15849-15886`): fills ticket/thumbnail/`thumbnail_handle` and moves the card to `DownloadState::Shared { name, path, size: None }` (`app.rs:15873-15877`).

### Rendering path (shared by in/out)
- `view_chat_log` (`app.rs:28189`): for `ChatKind::System` entries with `entry.download`, `self.view_download_attachment(i, dl)` (`app.rs:28771-28774`) is wrapped as `Row::new().push(dl_el).width(Length::Shrink).padding(right SPACE_12)` (`app.rs:28778-28781`). There is **no `.max_width(...)`** on this row (contrast text bubbles: `bubble_col.max_width(bubble_max_w)` at `app.rs:28618`; `bubble_max_w = chat_bubble_max_width(timeline_width)` at `app.rs:28267`, definition `examples/iced_chat/presentation.rs:27-33` = `min(560px, 68% of timeline)`; constants `design_tokens.rs:174-177`).
- `view_download_attachment` (`app.rs:7938-7978`) with `video-playback` delegates to `crate::download_progress_view::view_download_progress_with_player(entry_index, attachment, dark_mode, player, preparing, seek_position, expanded)` (`app.rs:7962-7970`).
- `download_progress_view.rs` builds the whole card in `view_download_progress_inner` (`:273-737`).

---

## 3. Existing state model

### `DownloadState` (`app.rs:1832-1868`)
```rust
enum DownloadState {
    Ready    { total: Option<u64> },                       // card shown, not yet downloading
    Active   { bytes: u64, total: Option<u64> },           // transferring
    Paused   { bytes: u64, total: Option<u64> },           // user-initiated pause
    Completed{ saved_name: String, saved_path: Option<PathBuf>, total_size: Option<u64> },
    Shared   { name: String, path: PathBuf, size: Option<u64> }, // local user's own file
    Failed   { failure: DownloadFailure },
    Cancelled,
}
```
- `is_terminal()` (`app.rs:1870-1879`): `Completed | Shared | Failed | Cancelled` — late progress events cannot overwrite terminal states.
- `DownloadFailure` (`app.rs:1618-1642`): `PermissionDenied`, `FileRemoved`, `FileChanged{detail}`, `VersionMismatch{current_version, detail}`, `SourceUnavailable{detail}`, `PeerOffline{detail}`, `VerificationFailed{attempts, max_attempts, detail}`, `Other{detail}`. Helpers: `message()` (`app.rs:1680+`), `recovery_action()` (`:1785`), `stability_label()` (`:1798` → Temporary/Terminal/Permanent), `retry_available()` (`:1809`), `diagnostics()` (`:1818`).
- `VideoPresentationState` (`download_progress_view.rs:114-122`): `Remote | Downloading | Verifying | Ready | Failed | Missing`. Mapping `video_presentation_state(attachment)` (`:124-147`): `Ready|Cancelled → Remote`; `Active|Paused → Downloading`; `Completed{saved_path:None} → Verifying`; `Completed|Shared` with existing path `→ Ready`; missing path / `Failed::FileRemoved → Missing`; other `Failed → Failed`.

### Transition triggers (all in `app.rs`)
- `Ready` ← `set_pending_file` (`:21457`) / `new()` (`:1975`).
- `Active{0,total}` ← `ExecuteDownloadAt` → `download_blob_to_file` (`:15270-15281`), upload card at `:14597-14600`; progress updates via `DownloadProgress` → `handle_download_progress` (`:7614+`).
- `Paused` / `Active` ← `PauseDownloadAt` / `ResumeDownloadAt` (`download_progress_view.rs:870-881`).
- `Completed` ← `DownloadDone` (`:15054-15058`) / `DownloadDonePeerFile` (`:15107-15111`).
- `Failed` ← `DownloadFailed` → `DownloadFailure::from_error` (`:15179-15181`); also `refresh_missing_downloads` (`:7907-7930`, Completed path gone → `FileRemoved`) and `OpenDownloadedFile` failure (`:15636-15652`).
- `Cancelled` ← `CancelDownloadAt`.
- `Shared` ← `FileDownloaded` for local uploads (`:15873-15877`).

---

## 4. Existing actions (rendered in `download_progress_view.rs::action_buttons` `:836-912`)

| State | Buttons → `AppMessage` |
|---|---|
| `Ready` | `Download` → `ExecuteDownloadAt(entry_index)` |
| `Active` | `Pause` → `PauseDownloadAt`, `Cancel` → `CancelDownloadAt` |
| `Paused` | `Resume` → `ResumeDownloadAt`, `Cancel` → `CancelDownloadAt` |
| `Completed` / `Shared` | `Open` → `OpenDownloadedFile(name)`, `Re-share` → `ReshareFile(entry_index)` |
| `Failed` (retryable) | `Retry` → `ExecuteDownloadAt`, `Remove` → `CancelDownloadAt` |
| `Failed` (terminal) | `Remove` → `CancelDownloadAt` |
| `Cancelled` | `Retry` → `ExecuteDownloadAt`, `Remove` → `CancelDownloadAt` |
| Video specials | `Completed{saved_path:None}` → disabled `Verifying…`; `Completed` path missing / `Failed::FileRemoved` → `Download` |

Other always-present chrome:
- `Open downloads folder` button below every card (`download_progress_view.rs:695-702`) → `AppMessage::OpenDownloadsFolder` (`app.rs:18858-18868`, opens `data_dir/downloads` via `open::that`). This is the "inconsistent blue button" the spec wants replaced (it uses the default iced button style — no custom styling).
- `Open File` handler: `OpenDownloadedFile` (`app.rs:15636-15656`) → `open_downloaded_file` (`app.rs:7815-7903`): resolves `Completed.saved_path` → `Shared.path` → `boru_downloads_dir/name` → cwd; launches `xdg-open` / `open` / `cmd /C start` per OS; missing file → `Failed::FileRemoved`.
- `Re-share`: `ReshareFile` (`app.rs:15657-15675`) re-encodes `saved_name|saved_path|saved_path` into `ExecuteFileSend`.
- Play: `PlayInlineVideo` (`app.rs:15196-15420`) — gates on `video_runtime.available`; only plays from `Completed{Some(path)}` or `Shared{existing path}`; if not downloaded yet it starts the download and tells the user to click play again; verifies via `verify_local_attachment` against `expected_content_hash`; builds `iced_video_player::Video::new(file_uri)` in `spawn_blocking`; `InlineVideoEvent::Loaded` (`app.rs:15571-15595`) stores `Arc<Video>` in `InlineVideoSession`.
- Player controls inside the card: slider (seek), Play/Pause, Mute, volume slider, Expand/Collapse (`download_progress_view.rs:581-622`); `InlineVideoToggleExpanded` (`app.rs:15565-15568`) opens the overlay `view_expanded_inline_video` (`app.rs:22116-22193`).

---

## 5. Available video metadata

`DownloadAttachment` (`app.rs:1909-1932`):

| Field | Type | Notes |
|---|---|---|
| `kind` | `TransferKind` | `Image | Video | File` (from `classify_attachment`, extension-based) |
| `name` | `String` | display filename (extension hint only; no MIME type in the card) |
| `ticket` | `String` | serialised `BlobTicket` |
| `transfer_id` | `Option<TransferId>` | set when transfer starts |
| `state` | `DownloadState` | see §3 |
| `source_peer` | `String` | sender display name / short key — **empty for incoming files** (`app.rs:21452`) |
| `speed_bytes_per_sec` | `Option<u64>` | periodic transfer speed |
| `thumbnail` | `Option<Vec<u8>>` | poster bytes (sender-generated WebP, ≤512 KiB, ≤320 px edge via `video_poster`) |
| `thumbnail_handle` | `Option<iced::widget::image::Handle>` | cached handle (decode-once) |
| `thumbnail_hash` | `Option<MessageHash>` | poster blob hash in `Message::FileShare` — **stored but never fetched** (gap G4) |
| `poster_dimensions` | `Option<(u32,u32)>` | decoded from the *poster* bytes, not the video |
| `playback_error` | `Option<InlinePlaybackError>` | set when player creation/decoding fails |
| `expected_content_hash` | `Option<String>` | blob hash parsed from ticket in `new()` (`app.rs:1960-1963`) |

Dimensions:
- `poster_dimensions` is computed in `DownloadAttachment::new` from thumbnail bytes via `image::ImageReader::into_dimensions` (`app.rs:1964-1969`), in `ThumbnailFetched` (`app.rs:15897-15902`), and in `PosterGenerated` (`app.rs:15144`).
- **Intrinsic video width/height are NOT available anywhere in the card model.** `video_poster::generate` deliberately scales the poster to ≤320 px (`MAX_POSTER_EDGE = 320`, `src/video_poster.rs:8,59-66`), so `poster_dimensions` ≈ poster size, not source video size.

Duration:
- **Not carried in the model.** The only duration access is runtime `Video::duration()` from the live player: `download_progress_view.rs:576` (position/duration label in controls) and `app.rs:15527` (seek fraction). There is no duration badge on the poster and no persisted duration.

Size / sender / time:
- Size: state-dependent `total` (`Ready/Active/Paused/Completed`) or `Shared.size` (always `None` in the upload path, `app.rs:15876`).
- Sender: `source_peer` (empty for incoming).
- Timestamp: `DownloadAttachment` has none; the enclosing `ChatEntry.timestamp` is set to `now_ms()` in `system_download` (`app.rs:2433`). No "Received Xm ago" metadata is rendered.

---

## 6. Current aspect-ratio handling + why the card becomes too wide/tall

### Sizing helpers (`download_progress_view.rs:97-112`)
```rust
fn inline_video_preview_height(d: Option<(u32,u32)>) -> f32 {   // :98-104
    let (w,h) = d.filter(positive).unwrap_or((16,9));
    (360.0 / (w as f32 / h as f32)).clamp(120.0, 280.0)         // default 16:9 → 202.5
}
fn inline_video_preview_width(d: Option<(u32,u32)>) -> f32 {    // :107-112
    d.filter(positive).map(|(w,_)| (w as f32).clamp(160.0, 640.0)).unwrap_or(360.0)
}
```
Applied only for `kind == TransferKind::Video` (`:475`): poster image with `content_fit(ContentFit::Cover)` `:482`; frame `container(...).width(Fixed(preview_width)).height(Fixed(preview_height)).clip(true)` `:560-562`.

### Root causes of the excessive width/height
1. **No width cap on the card.** Card container is `.width(Length::Shrink)` (`:724-725`); the chat embedding row is also `Length::Shrink` with no `max_width` (`app.rs:28778-28781`). Text/image bubbles are capped at `min(560px, 68%)` (`app.rs:28618`), but the download card bypasses that cap. `preview_width` alone can reach 640 px, plus 2×`SPACE_16` padding, icon + badge + size row → the card routinely exceeds the 560 px bubble cap and can exceed narrow windows.
2. **Portrait/vertical videos get a wide fixed frame.** `inline_video_preview_width` clamps only the width (160–640) and `inline_video_preview_height` clamps only the height (120–280) — the two clamps are independent, so the box does **not** preserve aspect ratio. A 9:16 video (poster ~320×568) renders in a 320×280 box (if the sender poster is the standard ≤320 px) or up to 640 wide (if poster dims come from a larger thumbnail), i.e. a portrait card that is wider than it should be. There is no portrait/narrow-frame branch.
3. **`ContentFit::Cover` crops meaningful content** (`:482`). A 9:16 poster scaled to fill a 320×280 box loses ~50% of its height (only the middle band shows); landscape is cropped horizontally. The spec mandates `contain` + centred + never-crop.
4. **Long filenames widen the card.** The header filename text uses `.width(Length::Fill)` with `Wrapping::Word` (`:327-334`) inside a `Shrink` card; `Wrapping::Word` only breaks at spaces, so an unbroken long filename forces the Shrink container wider than the window. (Matches the spec's Task 4 complaint: "Do not allow a very long filename to widen the card".)
5. **Height blow-ups.** (a) The active-player controls row is a tall fixed block: seek slider + Play/Pause + time + Mute + volume slider + Expand (`:581-622`). (b) The player container is `.width(Length::Shrink)` with `ContentFit::Contain` (`:626-642`) and no height cap — a portrait video can render very tall (intrinsic ratio × resolved width) with no `max_height` anywhere in the media frame.
6. **Poster ↔ player geometry mismatch (Task 10 violation).** Poster: `Fixed(w,h)` + `Cover`; player: `Shrink` + `Contain`. Pressing Play replaces a 320×280 crop with a shrink-sized contain player → the card (and the chat layout) jumps; scroll position is disturbed.
7. **Duplicate system text.** The incoming path pushes a system chip `"<sender> shared a file: <name>"` (`src/chat_core.rs:1993`) *and* the download card (which shows the same filename prominently) → duplicated filename text above/beside the card (spec Task 12 complaint).

### Behaviour when dimensions are unknown
- No poster yet → placeholder `"VIDEO" / "Preview available after download"` fills the frame (`:487-502`); frame uses the 16:9 default (202.5 × 360) — a reasonable default, but it is not updated live (only via `PosterGenerated` after the download completes).
- `playback_error` present → error panel overlay with title/message + "Retry player" button (`:406-411`, `:530-547`).

---

## 7. Poster generation / loading
- `src/video_poster.rs`: `generate(path, cache_dir)` (`:32-92`) runs `ffmpeg -ss 0.5 -frames:v 1 -vf "scale='min(320,iw)':-2" -c:v libwebp -quality 80`, bounded by `MAX_POSTER_BYTES = 512 KiB` (`:6`) and `MAX_POSTER_EDGE = 320` (`:8`); content-addressed cache key = blake3 of file bytes (`:24-26`); returns `Poster { bytes, dimensions, cache_path }` (`:14-21`). Blocking — callers must `spawn_blocking`.
- Call sites: outgoing upload `app.rs:14641-14654`; download-complete `app.rs:15064-15080`, `15116-15131`; catalogue prewarm helper `app.rs:6446-6461`.
- Incoming pre-download: `thumbnail_hash` is stored (`app.rs:21458`) but there is **no fetch code** in the repo — `AppMessage::ThumbnailFetched` is declared (`app.rs:5651`) and handled (`app.rs:15887-15908`) but never emitted. Consequence: remote video cards show the placeholder until the local download finishes and a fresh poster is generated locally (gap G4).

---

## 8. Player vs thumbnail components
- **Separate components.** Thumbnail: `iced::widget::image(thumbnail_handle).content_fit(Cover)` (`:479-485`). Player: `iced_video_player::VideoPlayer::new(&video).content_fit(Contain)` (`:626-629`). They share the same entry function but differ in geometry (`Fixed` box vs `Shrink`) and content-fit (`Cover` vs `Contain`).
- Expanded overlay reuses the same `view_download_progress_with_player` inside a `FillPortion(9)/FillPortion(9)` panel (`app.rs:22142-22150`, `:22168-22169`).
- Player lifecycle: one active `InlineVideoSession` (`app.rs:137-149`), keyed by `VideoInstanceKey(topic, message_id, attachment_id)`; viewport-based pause/release (`reconcile_inline_video_viewport` `app.rs:8837-8908`); jitter buffer for frame timing (`VideoJitterBuffer`).

---

## 9. Shared components that can be reused by the redesign

- **Design tokens** (`design_tokens.rs`): `SPACE_2..SPACE_40` (`:128-140`), `RADIUS_SM/MD/LG/XL/CARD` (`:147-151`, `RADIUS_CARD = 16.0`), `BORDER_WIDTH = 1.0` (`:154`), `CHAT_BUBBLE_MAX_WIDTH = 560.0` (`:174`) + `CHAT_BUBBLE_WIDTH_RATIO = 0.68` (`:177`), `DASHBOARD_MAX_WIDTH = 1480.0` (`:225`), `card_style(theme)` (`:730-740+`, surface bg + border + `RADIUS_CARD` + shadow). Colour/type helpers re-exported into the card: `accent_green, accent_primary, bg_surface, border_muted, color_error, text_system` (`download_progress_view.rs:27-30`).
- **Typography** (`fonts.rs`): `TypeRole` (`:357-388`) — chat-facing roles use Figtree (`ChatMessage`, `ChatSender`, `ChatMetadata`, `ComposerText`), technical ids use JetBrains Mono (`TechnicalValue`), everything else Source Sans 3; helper `type_role_text(role, text)`.
- **Card shell** (`card_shell.rs`): `CardShell` builder (`:101+`) with header/subtitle/count badge/status pill (`StatusBadgeKind` `:66-75`)/footer; row-height tokens `CARD_ROW_HEIGHT`/`PEER_ROW_HEIGHT`/`DEFAULT_LIST_MAX_HEIGHT` (`:44-59`).
- **Icons** (`icon_system.rs`): `Icon` enum → Lucide SVG, `IconSize` (XS–XL), `.tooltip()` for accessible icon-only buttons; `icon_svg` app helper (used at `download_progress_view.rs:322`).
- **UI components** (`ui_components.rs`): `gutter_scrollable`, `badge_owned`/`BadgeKind`, `LoadingSkeleton`, `InlineError`, `empty_state`, `TableHeaderRow`.
- **Presentation** (`presentation.rs`): `chat_bubble_max_width` (`:27-33`) — the right cap to apply to the card.
- **Poster pipeline**: `boru_core::video_poster` (unchanged; still useful for poster bytes + dims).
- **Progress bar pattern**: `progress_section` (`download_progress_view.rs:747-833`) already builds a thin (6 px) rounded progress bar with percentage — reusable for the redesigned progress treatment.
- **Virtualization/perf**: `LayoutCache` + `iced::widget::lazy` wrapper (`app.rs:7938-7977`); keep the card lazy so progress events don't rebuild every row.

---

## 10. Key gaps / findings for the redesign cards (VIDCARD-02+)

- **G1 — No intrinsic video dimensions.** `poster_dimensions` are poster (≤320 px) dims only. The redesign must either read real `video_width/video_height` (e.g. extend `video_poster::generate` or a metadata probe to return source dims) or use poster dims as a best-effort proxy and treat unknown as 16:9 default.
- **G2 — Duration not available pre-playback.** No duration badge possible with the current model; a metadata probe (ffprobe) or runtime-only duration would be needed.
- **G3 — Card bypasses the bubble width cap; fixed-box preview + `Cover` crop; poster/player geometry differ; no portrait/narrow branch; no `max_height`.**
- **G4 — Incoming thumbnail never fetched** (`thumbnail_hash` stored, `ThumbnailFetched` never emitted) → remote cards show placeholder until local download completes.
- **G5 — Incoming cards have empty `source_peer`** → no "From:" line for received files.
- **G6 — Duplicate system text** ("<sender> shared a file: <name>" chip + card).
- **G7 — "Open downloads folder" is an unstyled default button** (spec wants it replaced).
- **G8 — Long unbroken filenames widen the card** (`Wrapping::Word` + `Length::Fill` in a `Shrink` card).

---

## 11. Baseline evidence
- Repo: `origin` → `dmahony/boru.git`, branch `wt/t_58b2cda9` (worktree `~/.worktrees/t_58b2cda9`).
- Analysis commit: `<commit to be recorded after landing>` — this note only; no production code changed.
- Build (acceptance criterion): `cargo build --bin boru --features gui,video-playback,terminal` — result recorded in the task handoff (exit code + duration).
