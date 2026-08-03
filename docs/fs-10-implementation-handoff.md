FS-10 "Connect native file and folder sharing actions" — implementation handoff
===============================================================================

CARD
----
FS-10 (t_5c3c3c46) — Phase C "Shared by Me" | Depends: FS-01, FS-03, FS-08, FS-09
Gate: Shared-by-Me gate | Risk: High

STATUS
------
IMPLEMENTED — module committed; app.rs wiring present in the working tree,
deferred per the established module-first pattern (same as FS-09/FS-12/FS-13/
FS-14). The working tree compiles and all 732 example tests pass.

SUMMARY
-------
The dashboard's green "+ Share Files or Folder" button now opens a compact
native-action menu with exactly "Share Files..." and "Share Folder...".
"Share Files..." reuses the existing secure share entry point
(`AppMessage::AddSharedFile` → `rfd::AsyncFileDialog::pick_file` → BLAKE3
content-addressed registration → signed catalogue), unchanged. "Share
Folder..." opens the native OS folder picker
(`rfd::AsyncFileDialog::pick_folder`); because Boru's secure catalogue is
strictly file-based (content-addressed `file_objects` + `shared_files` rows),
a picked folder is surfaced as an explicit, path-free limitation message —
never flattened into fake rows and never routed around authorization,
descriptors, expiry, or hash verification. Registration shows nonblocking
status under the card header ("Registering <name>…"), cancel is a no-op, and
success refreshes the table from the authoritative `refresh_shared_by_me`
projection rather than inserting a manual row.

The change is task-scoped: no chat-timeline code was rewritten, and the
existing chat composer attachment and Home "Share Files" quick action are
untouched.

CHANGED FILES (verified paths)
-----------------------------
- examples/iced_chat/shared_by_me_table.rs  (FS-10 module — COMMITTED)
    * SharedByMeUiState gains `share_menu_open: bool` and
      `sharing_status: Option<String>`; `clear()`, `toggle_share_menu()`,
      `close_share_menu()` updated; `toggle_menu()` now mutually exclusive
      with the share menu.
    * `SHARE_MENU_ITEMS` const: exactly ("Share Files...", AddSharedFile)
      and ("Share Folder...", AddSharedFolder).
    * `card_header` renders the compact menu; `view_shared_by_me_card`
      renders the nonblocking status line under the header.
    * 3 new tests (14 total in module, all pass).
- examples/iced_chat/app.rs  (FS-10 wiring — IN WORKING TREE, deferred)
    * New messages: `AddSharedFolder`, `SharedFolderPicked(String)`,
      `SharedByMeToggleShareMenu`, `SharedFileAddFailed(String)` (+ name arms).
    * Handlers: share-menu toggle; native folder picker with cancel→Noop;
      explicit folder limitation (display name only, no full path);
      `SharedFilePicked` sets progress status + closes menu, errors map to
      `SharedFileAddFailed` (clears status); `SharedFileAdded` clears status
      and chains `refresh_shared_by_me()` so the table updates from the
      authoritative projection.
- Unblocking compile fixes (IN WORKING TREE, not FS-10 logic; required so the
  shared tree builds — the tree had 17 pre-existing errors from concurrent
  sibling work): app.rs (14) and main.rs (3) mechanical fixes; plus
  `badge_owned` in ui_components.rs and one test-field fix in
  recent_activity_view_model.rs. These are documented for the serialization
  pass; none change runtime behaviour.

DESIGN / ARCHITECTURE DECISIONS
-------------------------------
1. Reuse, do not duplicate. The menu item for files emits the pre-existing
   `AddSharedFile` message; the registration pipeline (blocking read on
   `spawn_blocking`, BLAKE3 content hash, metadata_id, MIME sniff,
   `put_file_object` + `set_file_object_source_path` + `upsert_shared_file`)
   is byte-for-byte unchanged. No second sharing subsystem was introduced.
2. Folder = explicit limitation, not a new subsystem. The schema has no
   folder object (`shared_files` rows reference `file_objects` blobs), so a
   directory cannot enter the signed/authorized catalogue without inventing
   one. The native folder picker still opens (the OS provides the browser —
   acceptance criterion 1), and the picked folder yields a truthful,
   path-free system message. This satisfies "make the limitation explicit"
   and the orchestrator rule ("the dashboard adds management, not a second
   sharing subsystem").
3. Picker work stays off the UI thread: `rfd::AsyncFileDialog` is async and
   the file read/hash runs in `tokio::task::spawn_blocking` (existing
   behaviour preserved).
4. Cancel is a no-op: both pickers map `None` → `AppMessage::Noop`.
5. Authoritative refresh: success chains `refresh_shared_by_me()` (the FS-09
   projection task) instead of manually inserting a row.
6. Privacy: progress and messages carry only display names; full local paths
   are never logged at normal levels or rendered. The folder-limitation copy
   uses the folder's file name only.

COMMANDS RUN (exact + result)
-----------------------------
- cargo build --example boru --features gui            → OK (Finished dev profile)
- cargo test --example boru --features gui shared_by_me → 14 passed; 0 failed
- cargo test --example boru --features gui             → 732 passed; 0 failed
- timeout 12 xvfb-run -a ./target/debug/examples/boru --data-dir /tmp/fs10-smoke
  --no-dht --no-relay                                  → EXIT 124 (ran until
  timeout; no panic/error in log; only benign libEGL DRI3 warnings under
  headless Xvfb)

TESTS
-----
- New (3): share_menu_toggle_is_mutually_exclusive_with_row_menus,
  share_menu_items_are_exactly_files_and_folder,
  sharing_status_is_cleared_with_the_rest_of_ui_state.
- Existing shared_by_me_table tests still pass (11 → 14 total).
- Full example suite: 732/732 pass (includes dashboard, projection,
  catalogue, chat regression tests).

RUNTIME / MCP EVIDENCE
----------------------
GUI smoke test under Xvfb: the binary boots, initialises the Iced shell, and
stays alive until the timeout; no panic, no error output. Full interactive
picker selection cannot be automated in this headless environment — manual QA
on a desktop session is required to click through the native dialogs
(Linux/GTK, macOS, Windows).

VISUAL EVIDENCE
---------------
None captured in this run (headless). Recommend a desktop-session screenshot
of the open share menu and the in-card status line during manual QA.

SECURITY / PRIVACY IMPACT
-------------------------
- No new attack surface: no new network calls, no new storage, no new
  permissions; the same `rfd` native picker the app already used for files is
  now also used for folders.
- No paths leak: status line and limitation message use file/folder display
  names only; registration still content-addresses and hashes the file
  exactly as before.
- Folder selection deliberately does NOT bypass the secure flow — it exits
  with an explicit limitation instead of faking access.

KNOWN LIMITATIONS
-----------------
- Folder sharing as a unit is not implemented (requires a schema/collection
  change — see FOLLOW-UPS). The picker exists; the limitation is explicit.
- Multi-file selection is not offered: the existing secure workflow
  registers one path per message; batch registration would need a new
  progress/error surface. Single-file selection matches existing semantics.
- The FS-10 app.rs wiring is uncommitted (deferred with the rest of the
  Phase C wiring per the module-first pattern). The module commit alone does
  not light up the menu until the wiring lands in the serialization pass.

FOLLOW-UPS
----------
- Serialization pass: commit the FS-10 app.rs wiring (new messages/handlers)
  together with the other deferred Phase C wiring.
- Product decision: add a folder/collection object to the schema if
  folder-as-a-unit sharing is wanted; the picker plumbing is ready.
- Manual QA matrix: file, multiple-file (N/A — limitation documented),
  folder, cancel, permission denied, missing file, large file, hashing
  failure, on each desktop OS.

COMMIT
------
See git log: commit referencing FS-10 (module change).
