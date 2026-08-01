# Inline video playback: message and attachment architecture

Step 2 of the inline-video implementation guide. This note records the existing extension points; it does not introduce a second attachment model or decoder state into persistent records.

## Executive map

```text
Gossip bytes
  -> src/chat_core.rs::forward_gossip_events_with_safety
  -> SignedMessage::verify_and_decode
  -> NetEvent::Message { from, message, sent_at }
  -> IcedChat::process_net_event_sync
  -> chat_core::handle_net_event_with_safety_for_topic
  -> ChatCallbacks implementation in src/chat_core.rs
  -> AppState pending queues / IcedChat update messages
  -> examples/iced_chat/app.rs::ChatEntry
  -> IcedChat::view_chat_log (virtualized scrollable)
```

The same GUI update/view path is used for room and direct-message conversations. The active conversation is selected by `TopicId`; direct-message topics are derived by `direct_topic` and represented in `ConversationStore`/`ConversationEntry`. There is no separate direct-message renderer.

## 1. Received attachment flow

### Wire decode and dispatch

- `src/api.rs:357-368` defines the gossip transport `api::Message` containing raw `Bytes`, delivery scope, and delivering endpoint.
- `src/chat_core.rs:867-998` defines the signed application protocol `chat_core::Message`.
  - `Message::FileShare { name, ticket, size, thumbnail }` announces a downloadable generic file. The ticket is a serialized `BlobTicket`; the protocol currently carries no MIME type or content hash for this variant.
  - `Message::ImageShare { name, hash }` announces an image by its blob/message hash.
- `src/chat_core.rs:1077-1122` defines `SignedMessage`; `verify_and_decode` verifies the sender signature before postcard-decoding the application message.
- `src/chat_core.rs:2115-2160` (`forward_gossip_events_with_safety`) performs the decode and emits `NetEvent::Message`.
- `src/chat_core.rs:1595-2047` (`handle_net_event_with_safety_for_topic`) processes the decoded event. For an accepted remote file it calls `ChatCallbacks::set_pending_file` (with name, ticket, size, thumbnail); for an accepted remote image it calls `set_pending_image` (name, hash, sender).

### GUI handoff and local readiness

- `src/chat_core.rs:518-556` stores the core pending queues (`pending_file`, `pending_image`). The GUI callback implementation is in `src/chat_core.rs:735-765`.
- `examples/iced_chat/app.rs` mirrors these as `IcedChat::pending_file` and `IcedChat::pending_image` (`app.rs:2134-2142`), and owns `download_entry_index`, `active_download_transfer_id`, and `transfer_id_to_index` (`app.rs:2149-2154`).
- File-share entries become visible as `ChatEntry::system_download` / `DownloadAttachment` cards (`app.rs:1797-1841`, `app.rs:1337-1369`). The current card is presentation state: `kind`, display name, ticket, optional `TransferId`, `DownloadState`, source label, speed, and optional thumbnail (`app.rs:1337-1350`).
- A received image is downloaded by the image task around `app.rs:4800-4847`. `download_blob_with_safety` reads the blob, the bytes are compressed and saved through `ImageStore::save_image`; the resulting relative `image_identifier` is passed in `AppMessage::ImageDownloaded`.
- Image local-file readiness becomes true at `AppMessage::ImageDownloaded` (`app.rs:11222-11306`): the bytes and saved identifier are available, `ChatEntry::image` creates a cached Iced image handle, and the entry is added to `IcedChat::entries`.
- Generic file local readiness becomes true only after `download_blob_to_file` completes (`app.rs:10932-11007`) and `AppMessage::DownloadDone` / `DownloadDonePeerFile` stores `DownloadState::Completed { saved_path: Some(path), ... }` (`app.rs:11089-11140`). The file download path is `data_dir/downloads/<name>` (`app.rs:10978-10988`). Before that, `DownloadState::Active` and progress are UI state only.
- Transfer progress is emitted through `src/chat_callbacks.rs:23-...` (`TransferId`, `TransferProgress`) and queued in `IcedChat::download_progress_queue` (`app.rs:2401-2405`). `AppMessage::DownloadProgress` calls `handle_download_progress` (`app.rs:4867-5055`), which updates the matching `DownloadAttachment` and invalidates layout. Completion is protected from late progress events by `DownloadState::is_terminal`.

### Verification and durable transfer state

- The durable storage model is in `src/storage.rs`, not in `ChatEntry`:
  - `FileObject` (`storage.rs:72-90`) has content hash, size, MIME hint, filename, optional inline data, and optional local `source_path`.
  - `MessageAttachment` (`storage.rs:106-119`) links a local `event_id` to a `file_objects.content_hash` and display filename/position.
  - `Download` (`storage.rs:216-241`) stores content hash, remote peer, state, byte counts, errors, and retry data.
  - `FileAvailability` (`storage.rs:282-300`) stores verification/availability, expected hash/size, and `verified_at_ms`.
- Schema definitions are in `Storage::migrate_v2` (`src/storage.rs:823-985`), with file verification in `migrate_v7` (`src/storage.rs:1051-1073`) and local source paths plus download temp/destination paths in `migrate_v8`/`migrate_v9` (`src/storage.rs:1075-1092`).
- `src/download.rs:1-136` is the verification boundary: `verify_download_file` checks regular-file status, exact advertised size, and BLAKE3 content hash; `verify_install_and_complete` installs only after verification and then marks the durable download complete. Decoder/player code must consume only this verified local result.
- `src/storage.rs:3879-4200` contains progress, completion, and lookup methods (`update_download_progress`, `complete_download`, `get_download`). `src/download_manager.rs:44-...` drives restart recovery and the queued/resolving/requesting/downloading/verifying lifecycle.
- Important gap for later steps: the existing `Message::FileShare`/`DownloadAttachment` GUI path uses a ticket and filename and does not currently create a `Storage::MessageAttachment` row or carry a content hash/MIME into `ChatEntry`. The existing durable model should be extended/reused rather than adding another attachment struct.

## 2. Sent attachment flow

### File picker and generic/video file

- `examples/iced_chat/app.rs:21331-21343` renders the paperclip button; it emits `AppMessage::AttachPressed`.
- `app.rs:9463-9489` opens `rfd::AsyncFileDialog`, keeps the basename and path, and routes image-looking names to `ExecuteImageSend`; all other names (including video names) go to `ExecuteFileSend`. This is currently extension-based presentation classification, not MIME/content probing.
- `app.rs:10706-10814` handles `ExecuteFileSend`:
  1. validates/reads metadata for the selected local path;
  2. marks `pending_file_upload` and creates an immediate upload card;
  3. classifies video names with `ChatEntry::is_video_file` and optionally runs `ChatEntry::generate_video_thumbnail` via `ffmpeg` (`app.rs:1843-1876`);
  4. streams the local file into the Iroh blob store;
  5. creates `Message::FileShare` with name, ticket, size, and thumbnail;
  6. signs with `SignedMessage::sign_and_encode` and broadcasts.
- The sent upload card is `ChatEntry::system_download` with a local `TransferKind` and is changed to `DownloadState::Active` immediately (`app.rs:10734-10756`). `FileDownloaded` only adds a system notice (`app.rs:10885-10887`); the durable sent message path is not currently linked to a `MessageAttachment` row.
- A local echo of the broadcast can later match the same `message_hash(message)` in `IcedChat::process_net_event_sync` (`app.rs:14999-15023`), using `self_sent_events: HashMap<MessageHash, u64>` and delivery state transitions. The local upload card itself is still associated through `download_entry_index`/transfer state, not through the message hash.

### Image send

- `app.rs:10816-10919` handles `ExecuteImageSend`: checks the size, reads and optimizes to WebP, stores the blob, registers it through `Storage::register_chat_upload` as `image/webp`, creates `Message::ImageShare { name, hash }`, signs/broadcasts, and returns `AppMessage::ImageDownloaded` so the sender gets the same inline image rendering path.
- `AppMessage::ImageDownloaded` adds a `ChatEntry::image` and a `HistoryEntry` (`app.rs:11222-11306`). Its image bytes are in-memory (`HistoryEntry::image_bytes` is `#[serde(skip)]`); the persisted `image_identifier` is the relative ImageStore reference.

## 3. Message identity and attachment/player keys

There are three related identities and they must not be conflated:

1. `MessageHash = [u8; 32]` (`src/chat_core.rs:1037-1046`) is the stable content hash of the decoded application `Message`. It is used by edits, deletes, reactions, read receipts, `message_hash_to_index`, and local echo/delivery matching. It is stable across room/direct rendering, but changes if the message payload changes.
2. `HistoryEntry::event_id: u64` (`src/chat_history.rs:168-203`) is a locally assigned monotonically increasing persistence id. `ChatEntry::event_id`, `event_id_to_index`, and outgoing SQLite state use it. It is stable locally across delivery-state transitions, but is not a portable peer identity.
3. `Storage::MessageAttachment::id: i64` is the durable attachment-row id; its `event_id + content_hash` relationship points at the content-addressed `FileObject`. `FileObject::content_hash` is the best content-level key for verified media shared by more than one message.

For one-active-video coordination, the later UI implementation should use a stable attachment/player key derived from the existing identity (prefer the durable attachment row/content hash once the file-share path is connected). Do not key a decoder by a transient vector index, `download_entry_index`, or a view/widget instance. For image/video messages where only `MessageHash` currently exists in the GUI, `MessageHash` is the available interim message key.

## 4. Persistence inventory

| Concern | Existing location/type | Current status |
|---|---|---|
| MIME type | `Storage::FileObject::mime_type`, `file_objects.mime_type`; catalogue `RemoteSharedFileRow::mime_type` | Present for content-addressed storage/catalogue; not present in `Message::FileShare` or `DownloadAttachment`. |
| Filename/extension | `FileObject::filename`, `MessageAttachment::display_filename`, `DownloadAttachment::name`, `HistoryEntry::text_preview` | Filename is present. Extension is only a hint; current GUI video detection uses extension and must not replace MIME/content probing. |
| Local path | `FileObject::source_path`; downloads `temp_path`/`destination_path`; GUI `DownloadState::Completed.saved_path`; ImageStore relative `image_identifier` | Available after a verified file install for generic downloads; image path is an ImageStore identifier rather than an absolute path. |
| Content address | `FileObject::content_hash`; `MessageAttachment::content_hash`; `Message::ImageShare::hash`; blob ticket hash | Present in the durable/content path and image protocol. Generic `FileShare` currently exposes only the ticket, whose blob hash is parsed at download time. |
| Progress | durable `Download.bytes_downloaded/total_bytes`; GUI `TransferProgress`, `DownloadState`, `CatalogueDownloadState` | Both durable and GUI progress paths exist; the chat card currently uses the GUI path. |
| Verification | `FileAvailability`, `download::verify_download_file`, `verify_install_and_complete`, download state `verifying/complete` | Explicit durable verification exists for the managed download pipeline. The direct GUI `download_blob_to_file` path must be checked/bridged before video playback is allowed. |
| Persistent chat metadata | `HistoryEntry` (`event_id`, hash, sender, topic, kind, preview, signed bytes, image identifier) | Images have replay support; generic file/video cards are not serialized as attachment records here. |

## 5. Iced update/view extension points

- `examples/iced_chat/app.rs` contains `AppMessage`, `IcedChat::update`, state mutation, and all chat view functions. Relevant message variants include `AttachPressed`, `ExecuteFileSend`, `ExecuteImageSend`, `ExecuteDownloadAt`, `DownloadProgress`, `DownloadDone`, `DownloadDonePeerFile`, `ImageDownloaded`, `OpenDownloadedFile`, `ReshareFile`, and `RetryOutgoingMessage` (the enum is around `app.rs:2940-3340`; update handlers are around `app.rs:9400-11320`).
- `ChatEntry` (`app.rs:1521-1596`) is the current GUI presentation record. It already owns image handles/bytes, dimensions, image-store identifier/errors, `DownloadAttachment`, delivery state, sender, `MessageHash`, and `widget_gen`. Decoder/player objects must remain outside this record as ephemeral UI/runtime state.
- `view_chat_panel` (`app.rs:18793-19292`) composes the header, `view_chat_log`, and composer. `view_composer` (`app.rs:21331-...`) owns the attach interaction.
- `view_chat_log` (`app.rs:20661-21329`) builds an Iced `scrollable` with id `CHAT_LOG`, scroll callbacks (`AppMessage::Scrolled`), cached layout, and a virtualized `[first_idx..=last_idx]` window. It reconstructs the visible message widget tree on each view pass; `ChatEntry::image_handle`, formatted caches, `LayoutCache`, and `widget_gen` avoid unnecessary decode/layout work, but the current code does not retain decoder objects in the widget tree.
- Attachment rendering is currently split: generic/file attachments are rendered by `view_download_attachment` for `ChatKind::System` (`app.rs:21119-21149`), while image handles are appended after the message row (`app.rs:21161-21225`). A static inline video card/player should extend this same `ChatEntry`/`view_chat_log` path rather than add a parallel message list.

## Later-step constraints

- Reuse `Storage::FileObject`, `Storage::MessageAttachment`, `Storage::Download`, and verification APIs; do not introduce another persistent attachment model.
- Keep decoder/player handles in an ephemeral `IcedChat` playback coordinator keyed by a stable message/attachment key.
- Gate playback on verified, locally available media. A filename extension or a received thumbnail is not proof that the video payload is safe or decodable.
- Preserve the existing virtualized scroll behavior and recreate lightweight cards during view passes; only the active video should own expensive runtime resources.
