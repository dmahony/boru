FS-14 "Implement the Downloading tab" — implementation handoff
=====================================================================

Status: VIEW MODEL MODULE COMMITTED (see commit below); TAB WIRING PRESENT IN
WORKING TREE, uncommitted because app.rs/main.rs are shared with concurrent
sibling workers (FS-09/11/13/15/17) whose half-done code currently blocks the
example build. Apply the wiring below in the serialization pass (or when
siblings land), then run the verification steps at the end.

CARD
----
FS-14 Implement the Downloading tab (t_cbee5d66)

SUMMARY
-------
The Downloading tab now renders the live FS-05 incoming-transfer projection:
each inbound transfer shows file/folder name (from the app's enrichment map,
falling back to a short stable id prefix — never a fabricated name or local
path), source peer (resolved from the authenticated peer id via resolve_name,
never a remote-supplied display string), bytes/total, a thin determinate or
indeterminate progress bar, truthful lifecycle state (Transferring / Retrying /
Verifying / Completed / Failed / Cancelled / Disconnected), started time, and —
only when they can be computed from two real observations — speed and ETA.

Design follows the dashboard style (white card, table/list rows, thin progress,
clear status, restrained actions) and the FS-12 precedent for a shared tree:
the self-contained, unit-tested view model module is committed; the app.rs
wiring (state fields, projection routing, tab view, cancel action, routing) is
in the working tree and documented below.

Only one action is offered: Cancel. Pause/Resume are NOT shown because the
FS-05 projection has no paused state and the backend cannot honour them for
the live inbound path (no fake "pause by hiding updates"). Cancel publishes a
Cancelled lifecycle event to the projection (authoritative state transition,
archived exactly once) and, when a durable download row maps to the same
content hash, calls the backend's real cancellation flow
(`DownloadManager::cancel_download` → `Storage::cancel_download`), with a
system message explaining partial-file handling (temp file removed on
cancellation; nothing kept).

COMMITTED ARTIFACT
------------------
examples/iced_chat/downloading_view_model.rs  (new, 540 lines, 8 unit tests)
  Commit: feat(FS-14): downloading tab view model for incoming transfers
  (see COMMIT section for hash)

Also committed (tiny additive lib API): `TransferState::is_terminal()` in
src/transfer_state_projection.rs — a public wrapper over the existing private
`terminal()`. This is required by the app wiring (outbound panel from FS-11's
handoff and this tab) and is strictly additive (existing 6 projection tests
still pass).

WIRING TO APPLY (in the serialization pass)
-------------------------------------------
1. examples/iced_chat/main.rs — declare the module (already added in the
   working tree, may be clobbered):
     mod downloading_view_model;

2. examples/iced_chat/app.rs — the following FS-14 pieces were added by this
   run and are present in the working tree at these approximate anchors;
   re-locate by symbol if the tree changed:
   - import: use boru_core::transfer_state_projection::{EventName,
     ProjectionUpdate, TransferDirection, TransferEvent, TransferRecord,
     TransferState, TransferStateStore, TransferUpdateReceiver};
     (also required by FS-11's outbound panel)
   - IcedChat fields (after outbound_history):
       inbound_item_labels: Arc<StdMutex<HashMap<String, String>>>,
       inbound_active: HashMap<String, TransferRecord>,
       inbound_history: VecDeque<TransferRecord>,
   - constructor init (after outbound_history init):
       inbound_item_labels, inbound_active, inbound_history,
     NOTE: like FS-11's outbound_* fields, these must be seeded from
     main.rs-created transfer_store/outbound_item_labels per the FS-11
     handoff; the serialization pass wires main.rs → IcedChat::new for both.
   - message variant: DownloadingCancel(String) + name()/log_variant arms
   - update() handlers: TransferProjectionUpdate routes by direction to
     apply_outbound_update/apply_inbound_update; TransferSnapshotResync calls
     resync_outbound_panel + resync_inbound_panel; DownloadingCancel calls
     cancel_inbound_transfer
   - state machine: apply_inbound_update, resync_inbound_panel,
     cancel_inbound_transfer (adjacent to apply_transfer_update /
     resync_outbound_panel)
   - views: view_downloading(), incoming_download_row() (adjacent to
     view_downloaded / downloaded_row)
   - routing in view_file_sharing: after the Downloaded-tab branch, add
       if active_tab == DashboardTab::Downloading {
           return scrollable(self.view_downloading())...;
       }

3. src/transfer_state_projection.rs — keep the committed is_terminal()
   addition; it unblocks both FS-11 and FS-14.

DESIGN / ARCHITECTURE DECISIONS
-------------------------------
- Live source of truth: the FS-05 projection store (TransferStateStore). The
  tab consumes inbound records only; the outbound card (FS-11) consumes the
  rest. No polling, no duplicate storage reads.
- Truthful states: IncomingState maps 1:1 from TransferState; Retrying is
  derived from a real attempt > 1, never guessed. Disconnected is NOT terminal
  (may resume); Completed/Failed/Cancelled move to the bounded history once.
- Progress: Determinate only when a positive total is known; otherwise an
  indeterminate bar + byte count; "Size unknown" when no bytes at all. No
  percentage is fabricated.
- Speed/ETA: computed only from two real observation deltas (bytes and
  updated_at_ms); a single sample yields no speed and no ETA.
- Peer identity: label is resolved from the authenticated peer id via the
  friend/name cache; never from an untrusted display field.
- Destination: no raw local path is rendered in rows. Completed rows are
  handled by the Downloaded tab (FS-15) with native Open/Reveal helpers.
- Cancel: publishes Cancelled to the projection (authoritative, archived once)
  AND calls DownloadManager::cancel_download for any durable row with the same
  content hash. Partial-file handling is explained to the user; the transfer
  layer removes temp files on cancellation.
- No native picker replacement: the tab is read-only live transfer state.
- No pause/resume: the projection has no paused state and the backend cannot
  honour it for the live inbound path.

COMMANDS RUN (exact + result)
-----------------------------
- Scratch-crate isolation (module included by #[path] against real boru-core):
    cargo test  (in /tmp/fs14-check)
    → 10 passed; 0 failed  (8 module tests + 2 harness tests)
- cargo test --lib transfer_state_projection
    → 6 passed; 0 failed (is_terminal addition is additive)
- cargo check --bin boru --features gui
    → still fails, but ONLY on sibling-owned errors (FS-11 blocked:
      OutboundState, transfer_store, outbound_*, MAX_OUTBOUND_HISTORY,
      PeerDownload, outbound_row, sort_outbound_rows; FS-16 clobbered:
      project_validated_remote_shared_file, remote_item_status,
      RemoteItemStatus; FS-15/UI-18 in flight: u64 deref, lifetimes, E0308,
      E0515, E0061). ZERO errors reference downloading_view_model /
      DownloadingCancel / view_downloading / incoming_download_row /
      apply_inbound_update / resync_inbound_panel / cancel_inbound_transfer.
      The 3 remaining inbound_* constructor-scope errors mirror FS-11's
      outbound_* constructor-scope errors exactly (both fixed by the
      serialization pass wiring main.rs → IcedChat::new).

TESTS
-----
downloading_view_model.rs has 8 unit tests covering:
- state mapping is truthful (Active→Transferring, peer id preserved)
- Retrying derived from real attempt>1, never guessed
- unknown total → indeterminate, no fabricated percentage
- speed requires two real observations (same-time/same-bytes/different-id/none
  all return None)
- ETA only when determinate + positive speed
- newest-first ordering with stable tiebreak
- byte-based formatting; missing data rendered explicitly
- projection reducer → truthful rows end-to-end

RUNTIME / MCP EVIDENCE
----------------------
None possible this run: the binary cannot be built while sibling
workers (FS-11 blocked, FS-09/13/15/17 running) hold the shared tree in a
non-compiling state. This mirrors FS-12's accepted status ("example blocked by
concurrent-worker code outside FS-12 scope") and FS-16's accepted status.
FS-23 is the integration card that will exercise the tab against real peers
via MCP.

VISUAL EVIDENCE
---------------
None this run (see above). The tab uses existing design tokens
(design_tokens::card_style, text_muted/primary, TableHeaderRow, badge,
ProgressBar, empty_state, SPACE_* grid) matching FS-08's shell, FS-07's
primitives, and FS-15's Downloaded tab.

SECURITY / PRIVACY IMPACT
-------------------------
- Rows never render raw public keys, content hashes, or local paths.
- Peer labels come from the resolved friend/name cache, never from an
  untrusted display string.
- Display names fall back to a short stable id prefix, never a path.
- Cancel only targets the authenticated local transfer and its own storage
  rows (matched by content hash); no cross-peer action.
- No new storage of sensitive fields; the tab reads the live projection.

KNOWN LIMITATIONS
-----------------
- Tab wiring (app.rs/main.rs) is uncommitted; it lives in the shared working
  tree and may be clobbered by sibling rewrites before the serialization pass —
  re-apply per the WIRING section above.
- The inbound projection only contains records that were published by the
  transfer engine. The app currently publishes inbound progress from the chat
  download path; the serialization pass should ensure TransferProgress events
  from every inbound path feed publish_progress so the tab is complete.
- Visual verification deferred to FS-23 (integration) once the example builds.
- FS-11's outbound panel (constructor params, main.rs store creation,
  subscription) is blocked; the Downloading tab shares the same store and will
  be live once that wiring lands.

FOLLOW-UPS
----------
- Serialization pass: commit the app.rs/main.rs FS-14 wiring once siblings
  land (and wire main.rs → IcedChat::new for transfer_store +
  outbound/inbound item labels per FS-11 + FS-14 handoffs).
- FS-22 (test coverage) should add the downloading_view_model tests to the
  main suite (they currently run via scratch-crate isolation because the
  example crate is blocked).

COMMIT
------
feat(FS-14): downloading tab view model for incoming transfers
(plus src/transfer_state_projection.rs is_terminal() additive API)
