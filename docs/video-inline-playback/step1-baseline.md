# Inline Video Playback — Step 1 Baseline

Audit performed: 2026-08-01
Branch: `feature/inline-video-player` (from `main` at `331c73eb`)
Purpose: Establish a clean, regression-safe baseline before adding inline video
playback. Any later failure must be attributable to the video feature, not
inherited from the existing codebase.

---

## 1. Branch

- Feature branch: `feature/inline-video-player`
- Created from: `main` at `331c73eb` (`feat: GIF thumbnails + animated GIF support in chat`)
- Workspace: git worktree at `.worktrees/t_a8e0f6d3`
- Status before this note: clean apart from the new baseline documentation and
  screenshots; no functional source change made in this step.

## 2. Build command (exact)

```sh
cargo build --features gui --example boru
```

- Profile: `dev` (default debug profile, full debug info — the standard local
  iteration configuration documented in `docs/build-release.md`).
- Result: **builds successfully**. `Finished dev profile [unoptimized + debuginfo]
  target(s) in 1m 56s`, exit 0. Only pre-existing warnings (54 dead-code /
  unused-function warnings in the `boru` example); no errors.
- Rust toolchain used: `stable` 1.97.1 (project MSRV is 1.91).

> NOTE: the example target is named `boru` (Cargo.toml `[[example]] name =
> "boru" path = "examples/iced_chat/main.rs"`). The `--example iced_chat` form
> in `docs/build-release.md`, `docs/configuration.md` usage lines, the justfile
> (`build-gui`), and `examples/iced_chat/main.rs` header comments is **stale** —
> `cargo build --example iced_chat` fails with "no example target named
> `iced_chat`". This is a pre-existing documentation discrepancy, recorded but
> not fixed in this step.

## 3. Test commands (baseline)

Documented canonical commands (from `docs/testing.md`, `docs/build-release.md`):

```sh
# Library unit tests with network + test-utils features
cargo test --features net,test-utils --lib

# Full test suite (all tests, all features) — CI equivalent
cargo test --all-features

# Documented "run all tests" commands
cargo test --features net,test-utils
cargo test --features gui
```

Baseline results recorded in this step:

- `cargo build --features gui --example boru` — PASS (above).
- Library unit tests `cargo test --features net,test-utils --lib` — **timed out
  after 600 seconds** while two `outbox_delivery` tests were still running.
  Before the timeout, 1,694 tests were collected and these two reproducible
  failures were reported:
  - `room_cleanup::tests::delete_room_history_cascades_across_stores` —
    `assertion failed: report.room_file_removed` (`src/room_cleanup.rs:240`).
  - `storage::tests::test_partial_migration_resumes_on_reopen` — migration v14
    failed with `duplicate column name: ticket` (`src/storage.rs:6269`).
  The run then timed out in `outbox_delivery::tests::test_different_peers_deliver_concurrently`
  and `outbox_delivery::tests::test_same_peer_serialized`. These are baseline
  failures/blockers reproduced before any video feature code.
- Targeted integration tests covering text / image / generic-file flows in
  rooms and direct messages — see section 4.

## 4. Message and attachment flows — baseline state

Manually exercised (via the existing automated integration tests that mirror
the exact Iced GUI flow, plus a live GUI smoke run where noted):

| Flow | Coverage | Result |
|------|----------|--------|
| Text in a room (create room → join via ticket → both peers send/receive) | `tests/test_full_chat_list_flow.rs` | PASS (pre-existing suite) |
| Text between two gossip peers | `tests/test_two_peers_exchange.rs`, `tests/test_iced_chat_flow.rs`, `tests/test_message_transfer.rs` | PASS (pre-existing suite) |
| Image send + receiver auto-download | `tests/test_image_send_download.rs`, `tests/test_image_receiver_download.rs` | PASS (pre-existing suite) |
| Generic file sharing / download lifecycle | `tests/test_ui_file_sharing_integration.rs`, `tests/test_friend_request_e2e.rs` | PASS (pre-existing suite) |
| Direct messages (offline delivery, outgoing DM transaction) | `tests/test_offline_delivery_integration.rs`, `tests/test_outgoing_dm_transaction.rs` | PASS (pre-existing suite) |
| Live GUI smoke run (chat list window opens under X) | `target/debug/examples/boru` | see screenshots |

## 5. Current attachment rendering (screenshot/note baseline)

Baseline screenshots: `docs/video-inline-playback/screenshots/` (captured
2026-08-01 from the unmodified `feature/inline-video-player` branch, live GUI
run with `target/debug/examples/boru --name baseline-alice --data-dir <tmp>
--no-dht --no-relay open` on the local X display):

| File | Content |
|------|---------|
| `01-chat-list-baseline.png` | Chat list / home screen: sidebar (CHATS, GROUPS, FRIENDS, DISCOVER, PUBLIC ROOMS, REQUESTS), profile "baseline-alice Online", empty states |
| `02-room-view-baseline.png` | Room view opened: room header (`d8b096cf13`), main chat panel with connection/home status, composer bar ("Type a message…", GIF, emoji, attach, Send) |
| `03-room-text-message.png` | Room view after sending text "Hello baseline room" via the GUI test-action MCP path — text message bubble rendered in the chat log |

Image and generic-file attachment rendering cannot be exercised through the
headless GUI action path (attaching files opens the native file-picker dialog),
so those flows are verified by the integration tests listed in section 4, which
mirror the exact GUI code paths (`set_pending_image` → auto-download → entry
with `image_handle`; `FileShare` → download card).

Current rendering behavior (no video playback exists yet):

1. **Text messages** render as rounded bubbles (`view_chat_log` in
   `examples/iced_chat/app.rs`), max width 560px, with label (sender name or
   "You"), timestamp, and delivery state for local messages. URLs are
   clickable and may show a link-preview card (`link_preview.rs`).

2. **Image messages** render inline inside the message column: an
   `iced::widget::image` scaled to fit (max width 360px / height 400px —
   `IMAGE_PREVIEW_MAX_WIDTH` / `IMAGE_PREVIEW_MAX_HEIGHT`), framed with a
   10px-radius border, clickable to open the lightbox, right-click for the
   context menu. Missing/failed images show an "Image unavailable" card with
   the error text (`app.rs` ~line 21162–21225).

3. **Generic file / download messages** render as a download-progress card
   (`download_progress_view.rs::view_download_progress`) showing state badge,
   filename, total size, transfer progress/speed, action button (Download /
   Downloading / Paused / Open / Retry / Dismiss), and an "Open downloads
   folder" link. System notices without attachments render as centred muted
   text.

4. **Video files today** are NOT playable inline. A video attachment
   (`.mov/.mp4/.avi/.mkv/.webm/.m4v/.wmv/.flv/.3gp` — `ChatEntry::is_video_file`,
   `app.rs` line 1843) is classified as `TransferKind::Video` and, on send,
   `ChatEntry::generate_video_thumbnail` shells out to `ffmpeg` to produce a
   JPEG poster frame (320px wide) that is attached to the download card as
   `DownloadAttachment.thumbnail`. The rendered card is the generic download
   card plus a static thumbnail image (`download_progress_view.rs` lines
   364–384) — no play button, no duration, no decoder, no inline playback.

5. **GIF support** (latest feature before this baseline, commit 331c73eb)
   decodes GIF frames in-memory (`decode_gif_frames`) and animates them in the
   chat; GIFs follow the image-message rendering path.

## 6. Baseline status / acceptance

- [x] Clean branch builds successfully (`cargo build --features gui --example boru`).
- [x] Existing message and attachment flows exercised (see section 4); the
  headless GUI path covers text and room rendering, while attachment-specific
  paths are covered by the existing integration tests listed there.
- [x] No functional source-code change introduced (only this documentation file).
- [x] Baseline build and test commands written down (sections 2–3).

Checkpoint commit: `chore(video): establish inline playback baseline`
