# FS-00 Boru repository and runtime baseline

## CARD / STATUS

- Card: FS-00 — Create a repository and runtime baseline
- Status: Discovery audit complete; no product-code changes made.
- Audit date: 2026-08-03
- Repository: `/home/dan/iroh-gossip-chat`
- Package: `boru-core` 0.108.0, Rust edition 2021
- Iced: 0.14 (`Cargo.toml:120`)

## SUMMARY

Boru is a Rust/Iced desktop GUI binary named `boru`, bootstrapped from
`examples/iced_chat/main.rs`. `IcedChat` in `examples/iced_chat/app.rs` owns
application state, the `Screen` route enum, the `AppMessage` event vocabulary,
update reducer, subscriptions, and top-level view composition. Home is
`Screen::ChatList`; an active room is `Screen::Chat { topic }`. The top-level
view composes the fixed sidebar, screen-dependent main panel, optional details
panel, and overlays.

The existing file system is deliberately split between chat attachments and
profile/catalogue offers. SQLite storage uses content-addressed `file_objects`
with relationship tables for `message_attachments`, `shared_files`, permissions,
and durable `downloads` (`src/storage.rs:8-29`). Remote catalogue metadata does
not contain local paths, database IDs, blob tickets, or upload secrets
(`src/catalogue_model.rs:132-164`).

## VERIFIED SOURCE MAP

### Entry point, state, routing, shell, and design system

- `examples/iced_chat/main.rs:8-22`: GUI modules; `main.rs:446` is the process entry point.
- `examples/iced_chat/main.rs:99-110`: graphical-session requirement; recommends `xvfb-run` when no DISPLAY/WAYLAND_DISPLAY exists.
- `examples/iced_chat/main.rs:113-170`: CLI flags and commands, including `--data-dir`, `--no-dht`, `--no-relay`, `--mcp`, `--enable-gui-test-actions`, `open`, `join`, and `logs`.
- `examples/iced_chat/main.rs:617-619`: persistent `Storage::open` at the selected data directory.
- `examples/iced_chat/main.rs:1355-1363`: MCP/Iced diagnostic journal and GUI action channels.
- `examples/iced_chat/app.rs:2436-2458`: `Screen` enum. Main routes include `ChatList`, `Chat`, `FriendRequests`, `Settings`, `PeerProfile`, `PeerCatalogue`, `FriendProfile`, `Discover`, `Groups`, and optional `Terminal`.
- `examples/iced_chat/app.rs:3543-4250`: `AppMessage`, the central command/event vocabulary.
- `examples/iced_chat/app.rs:7812`: `IcedChat::update`, the central reducer.
- `examples/iced_chat/app.rs:18882-18900` and `app.rs:~18900-19000`: top-level view and screen composition.
- `examples/iced_chat/app.rs:19540-21205`: sidebar/header, Chats, Groups, join ticket, Discover, Public Rooms, Friends, and Requests sections.
- `examples/iced_chat/app.rs:21209-21840`: Home (`view_main_empty_state`) and its action grid/share strip.
- `examples/iced_chat/app.rs:21844-24600`: chat panel/header/log/composer and attachment rendering.
- `examples/iced_chat/design_tokens.rs:1-503`: palette, semantic colors, spacing, radii, control heights, typography sizes, avatars, and layout constants.
- `examples/iced_chat/ui_components.rs`: shared card, row, button, input, status, and empty-state primitives.
- `examples/iced_chat/card_shell.rs`: shared rail/card shell and 48 px row rhythm.
- `examples/iced_chat/presentation.rs`: date/grouping/relative-time/delivery presentation helpers.

### Existing sharing entry points and picker behavior

1. **Chat composer attachment**: `AppMessage::AttachPressed` at
   `examples/iced_chat/app.rs:11093-11120` calls
   `rfd::AsyncFileDialog::new().set_title("Select a file to share").pick_file()`.
   The selected path is converted into the existing send message path:
   image extensions become `ExecuteImageSend`; other files become
   `ExecuteFileSend`.
2. **Chat slash command**: `/whisper-file` is explicitly rejected at
   `app.rs:11050-11056` with “Direct file transfer is disabled; use the
   authorised file catalogue.”
3. **Profile/shared-file library**: `AppMessage::AddSharedFile` at
   `app.rs:16602-16617` uses the same native `rfd::AsyncFileDialog` file picker
   and maps the result to `SharedFilePicked`. The follow-on path at
   `app.rs:16619-16655` reads metadata/content asynchronously, computes BLAKE3,
   and prepares the storage/catalogue entry.
4. **Home quick action**: the Home “Share files” strip currently emits
   `OpenSettings` (`app.rs:~21515-21533`, also documented in
   `docs/ui-redesign/current-ui-map.md:139-141`). It is a semantic mismatch,
   not a hidden picker path.
5. **Home import action**: `ImportFriendFromFile` at `app.rs:8407-8414` uses
   `rfd::AsyncFileDialog::pick_file()` for a friend public-key file. This is
   identity import, not file sharing.
6. **Profile image picker**: `PickProfileImage` at `app.rs:17067-17075` also
   uses the native `rfd` picker, but is unrelated to file sharing.
7. **Drag-and-drop/context-menu/command audit**: no file-sharing drag/drop
   picker path was found in the inspected GUI code. Chat context-menu messages
   (`ContextCopyText`, `ContextCopyImage`, `CloseContextMenu`) are copy/dismiss
   actions, not share initiation. No separate folder-picker path was found;
   existing share initiation is file-based and native-OS mediated.

The native picker dependency is `rfd = 0.15`, optional under the `gui` feature
(`Cargo.toml:76, 202`). Preserve this behavior; do not replace it with an
in-app file browser.

### Catalogue, authorization, signatures, expiry, persistence, and transfers

- `src/catalogue_model.rs:132-264`: `RemoteSharedFile` is remote-safe metadata;
  validation rejects unsafe identifiers, path separators, control/format
  characters, invalid MIME types, oversized metadata, and unreasonable future
  timestamps.
- `src/catalogue_model.rs` (signed catalogue implementation after the remote
  entry model): `SignedFileCatalogue` signs the requester-specific catalogue;
  tests cover round-trip verification, tampered files/owner/revision, and
  malformed metadata rejection.
- `src/catalogue_protocol.rs:168-274`: versioned catalogue request/response
  types, pagination, `NotModified`, stable wire error codes, and signed-catalogue
  response. `CataloguePage::verify` is intentionally a no-op because the complete
  signed catalogue is the verification boundary; item content hashes are checked
  during download.
- `src/catalogue_handler.rs:1-15,163-188`: server authenticates the remote
  endpoint, applies the requester’s friend/permission view, builds a fresh
  requester-specific signed catalogue, and applies size/concurrency/abuse limits.
- `src/file_access_handler.rs:1-15,47-120`: request-time authorization handler
  and single-use `NonceStore`; nonce entries expire lazily and replay is rejected.
- `src/file_access_protocol.rs:161-180`: `DescriptorVerification` states,
  including signature, expiry, owner/requester, nonce, and content mismatch.
- `src/file_access_protocol.rs:182-299`: `sign_download_descriptor` and
  `verify_download_descriptor`; signed fields include owner, requester, shared
  file ID, blob hash/content hash, size, nonce, issue time, and expiry.
- `src/download_initiation.rs:118-263`: initiation requires a fetched/verified
  remote catalogue, valid metadata, and no conflicting durable download before
  inserting a queued SQLite download.
- `src/blob_transfer.rs:1-22,68-99,142-300`: streamed Iroh blob transfer,
  bounded 128 KiB copy buffer, cancellation/timeouts, periodic progress
  persistence, size verification, incremental BLAKE3 verification, and cleanup
  on failure/cancellation.
- `src/download_limits.rs:1-6,24-42,274-360,442-511`: global/per-peer queue
  limits, independent hash-verification permits, and coalesced progress writes.
- `src/storage.rs:216-233,401-590`: durable download state, SQLite ownership,
  startup recovery, temp-path recording, and crash recovery.

### Runtime/MCP diagnostics

- `examples/iced_chat/mcp_server.rs:1-38`: documented MCP tools include
  `boru_ping`, node/room/peer status, discovery events, Iced state/journal,
  GUI navigation/snapshot/wait tools, `boru_browse_peer_catalogue`, and
  `boru_download_file`.
- `mcp_server.rs:460-559,1062-1075`: JSON-RPC dispatch and GUI snapshot handler.
- `examples/iced_chat/gui_test_actions.rs:396-402`: GUI snapshot model begins
  with current screen and state fields; GUI actions are semantic routes into
  normal `AppMessage` handling.
- GUI diagnostics are loopback-only when GUI test actions are enabled; the
  server prints an explicit warning and requires loopback binding in this mode
  (`main.rs:149-156`, `mcp_server.rs:46-53`).

## COMMANDS RUN AND RESULTS

All commands were run from `/home/dan/iroh-gossip-chat` without changing
product code.

- `rustc --version` → `rustc 1.97.1 (8bab26f4f 2026-07-14)`.
- `cargo --version` → `cargo 1.97.1 (c980f4866 2026-06-30)`.
- `cargo fmt --all -- --check` → exit 0 from the tool, but reported formatting
  differences in the pre-existing modified `examples/iced_chat/app.rs`; no
  files were rewritten.
- `cargo check --features gui --bin boru` → PASS, exit 0; 3 library
  warnings and 112 GUI-example warnings (unused/dead code, unfulfilled
  expectations, deprecated legacy history save, and private-interface warnings).
- `cargo build --features gui --bin boru` → PASS, exit 0; same warning
  baseline class as check.
- `cargo clippy --features gui --bin boru --all-targets -- -A clippy::all`
  → FAIL, exit 101 in pre-existing test compilation:
  `tests/test_friend_ticket_persistence.rs:64` implements `set_pending_file`
  with `Option<Vec<u8>>`, while the current trait requires `Option<[u8; 32]>`.
  This is not a change made by FS-00.
- `cargo test --lib` → timed out after 600 seconds (exit 124) while two
  `outbox_delivery` tests were still running. The run reached 1,797 library
  tests and emitted multiple pre-existing failures, including chat-core image
  and name-fallback tests, `room_cleanup::delete_room_history_cascades_across_stores`,
  and `storage::test_partial_migration_resumes_on_reopen`. This is an unhealthy
  baseline; it must not be reported as a passing full suite.
- `timeout 15s xvfb-run -a target/debug/boru --data-dir /tmp/boru-fs00-data --no-dht --no-relay`
  → exit 124 because the application remained alive for the timeout; this is a
  successful GUI event-loop smoke test. Only a non-fatal llvmpipe/libEGL DRI3
  warning was emitted.

## RUNTIME / MCP EVIDENCE

A fresh temporary data directory was used. The command
`xvfb-run -a target/debug/boru --data-dir /tmp/boru-fs00-mcp
--no-dht --no-relay --mcp --enable-gui-test-actions --mcp-bind
127.0.0.1:18765` stayed alive under Xvfb. JSON-RPC calls over loopback returned:

- `boru_ping` → `{"pong":true}`.
- `boru_get_node_status` → valid v0.108.0 response with one active room and a
  monotonically reported diagnostic event sequence.
- `boru_get_gui_snapshot` → valid snapshot with GUI test actions enabled,
  active room state, and diagnostic/journal counters.

No full peer/file-transfer scenario was run: it requires a second authorized
peer and real catalogue/file fixtures. The MCP surface is present for that
follow-up verification.

## VISUAL EVIDENCE

Existing committed baseline captures were verified with `file` as valid PNGs:

- Home: `docs/ui-redesign/evidence/baseline/t_9ec8d24f_home_1280x800_baseline.png`
  (1280x800).
- Chat: `docs/ui-redesign/evidence/baseline/t_9ec8d24f_chat_1280x800_baseline.png`
  (1280x800).
- Existing file-share flow: `docs/ui-redesign/evidence/ui-11/t_d9f6a827_files_flow_1280x800.png`
  (1280x800), alongside the public-file dialog and Home captures in the same
  `ui-11` evidence directory.
- Additional current Home/chat responsive baselines are listed in
  `docs/ui-redesign/evidence/baseline-capture.log`.

These are repository evidence from the existing UI-01/UI-11 captures; FS-00
performed no visual or product-code edits.

## NO-CHANGE STATUS / COMMIT

At audit start, the worktree was already dirty with unrelated in-flight UI-13,
UI-15, and event-grouping evidence/code changes. The pre-audit status included
10 modified paths and 6 untracked paths; notably `examples/iced_chat/presentation.rs`
and several evidence/scripts were already modified. FS-00 did not edit or stage
those paths.

The only FS-00 file is this architecture note:

- `docs/fs-00-baseline.md`

The baseline source HEAD observed before this note was:

- `f698dd0f553c0a712b9150f1e60a91e21d2c4a48`

The FS-00 commit is intentionally separate from all pre-existing dirty worktree
content and references FS-00 in its message.

## DESIGN / ARCHITECTURE DECISIONS

- Preserve `Screen`, `AppMessage`, Iced update/subscription contracts, protocol
  handlers, SQLite storage, signed descriptors, expiry, nonce replay prevention,
  content-hash verification, and native `rfd` picker behavior.
- Treat Home “Share files” → `OpenSettings` as a known semantic mismatch to be
  addressed only by a later scoped card; do not silently change it here.
- Do not add a new picker, file browser, transfer protocol, persistence layer,
  or MCP endpoint as part of the baseline.
- Keep runtime diagnostics loopback-only by default; GUI mutation tools require
  explicit enablement and are rate-limited.

## SECURITY / PRIVACY IMPACT

No product behavior changed. The current design keeps local paths out of remote
catalogue metadata, gates catalogue views per requester, rechecks authorization
at request time, signs short-lived recipient-bound descriptors, rejects replayed
nonces, verifies size and BLAKE3 content identity, bounds transfer resources,
and defaults direct-address publication off. The temporary audit directories
were outside the repository and were removed after the smoke tests.

## KNOWN LIMITATIONS / FOLLOW-UPS

- Full library tests are currently not a clean baseline and did not finish within
  ten minutes; investigate failures independently before using the suite as a
  gate.
- Clippy all-targets is blocked by the existing `set_pending_file` trait mismatch
  in `tests/test_friend_ticket_persistence.rs`.
- Current visual evidence does not exercise a real remote authorized download;
  add a two-peer catalogue/descriptor/transfer fixture in a later runtime card.
- The Home share-files strip needs an explicitly approved behavior decision.
- No drag-and-drop or folder-sharing initiation path was found in the current
  GUI code; if later requirements need either, add it as a new behavior card.

## FOLLOW-UPS

Use this note as the source map for subsequent file-sharing cards. Start with the
existing `rfd` picker and `AppMessage` routes, then preserve the catalogue/access
handlers and durable transfer state machine rather than introducing parallel
abstractions.
