FS-12 "Recent Download Activity (by Others)" — implementation handoff
=====================================================================

Status: MODULE COMMITTED (bdf7a0a5); WIRING PRESENT IN WORKING TREE, uncommitted
because app.rs/main.rs are shared with concurrent sibling workers (FS-09/13/14/15/17)
whose half-done code currently blocks the example build. Apply the wiring below
in the serialization pass (or when siblings land), then run the verification
steps at the end.

CARD
----
FS-12 Implement recent download activity by other peers (t_8712d524)

SUMMARY
-------
The "Recent Download Activity (by Others)" dashboard card is implemented and
bound to the durable, privacy-filtered transfer-activity projection from FS-06
(`transfer_activity` table, `record_transfer_activity` / `list_transfer_activity`).
Actions are derived from real lifecycle event names (requested, authorized,
started, in progress, downloaded/completed, failed, denied, cancelled, paused,
resumed, verifying, queued) — a request is never presented as success. Statuses
are compact success/error/warning/info with icons AND accessible text labels.
Rows are newest-first, deduplicated by event id (storage INSERT OR IGNORE +
projection HashSet), bounded to 50, and fall back to safe historical labels
("Remote peer" / "Shared item") when the underlying download/file row has been
removed or pruned — never exposing raw paths, hashes, or peer keys.
"View full activity log" selects the Activity Log tab
(`DashboardTabSelected(DashboardTab::ActivityLog)`).

COMMITTED ARTIFACT
------------------
examples/iced_chat/recent_activity_view_model.rs  (new, 389 lines, 12 unit tests)
  Commit bdf7a0a5 "feat(FS-12): recent download activity view model for dashboard card"

WIRING TO APPLY (in the serialization pass)
-------------------------------------------
1. examples/iced_chat/main.rs — declare the module (already added in the working
   tree, may be clobbered):
     mod recent_activity_view_model;

2. examples/iced_chat/app.rs — the following FS-12 pieces were added by the
   prior run and are present in the working tree at these approximate anchors;
   re-locate by symbol if the tree changed:
   - IcedChat field:            dashboard_recent_activity: Vec<crate::recent_activity_view_model::RecentActivityRow>
   - init:                      dashboard_recent_activity: Vec::new()
   - message:                   DashboardRecentActivityLoaded(Vec<crate::recent_activity_view_model::RecentActivityRow>)
   - name() arm:                AppMessage::DashboardRecentActivityLoaded(_) => "DashboardRecentActivityLoaded"
   - update() arm:              AppMessage::DashboardRecentActivityLoaded(rows) => { self.dashboard_recent_activity = rows; ... }
   - refresh task:              fn refresh_dashboard_activity(&self) -> iced::Task<AppMessage>
   - card view:                 fn view_recent_download_activity_card(&self) -> iced::Element<'_, AppMessage>
   - row view:                  fn recent_activity_row<'a>(&'a self, ...) -> iced::Element<'a, AppMessage>
   - refresh call sites:        self.refresh_dashboard_activity()  (on dashboard-open and on tab selection)
   - layout: replace the "Recent Activity" placeholder_card in BOTH the wide and
     narrow branches with:
         let recent_activity_section = self.view_recent_download_activity_card();

3. No storage changes are needed for FS-12: FS-06 already provides
   `record_transfer_activity`, `list_transfer_activity(limit)`, and
   `prune_transfer_activity`. The storage diff currently in the working tree
   (CompletedDownloadRecord, list_downloads, list_shared_peer_ids,
   list_completed_downloads) belongs to FS-13/FS-15, not FS-12 — do not commit
   it as part of FS-12.

DESIGN / ARCHITECTURE DECISIONS
-------------------------------
- Truthfulness: `normalize_event(event_name, payload_json)` maps lifecycle
  events to (action, status, detail). Only COMPLETION yields Success; FAILURE
  with error_category=permission_denied maps to "Denied" (grant refused or
  expired — the taxonomy cannot distinguish, so we don't invent which);
  CANCELLATION/PAUSE map to Warning; everything else Info. Unknown future event
  names render as neutral "Activity" notices — never reinterpreted.
- Dedup: storage INSERT OR IGNORE on event_id (primary key) + belt-and-braces
  HashSet in `project_recent_activity` for callers that feed unpersisted
  streams.
- Ordering: newest-first on occurred_at_ms with stable event-id tiebreak.
- Safe fallbacks: enrichment is keyed by short transfer_id; missing rows fall
  back to neutral labels. No raw peer key, content hash, or path is rendered.
- Retention: card shows up to MAX_RECENT_ACTIVITY_ROWS (50) of the storage's
  bounded (1000) projection. Empty state explains the retention window without
  implying sharing is broken.
- No native picker replacement: the card is read-only history; file sharing
  still uses the OS picker unchanged.

COMMANDS RUN (exact + result)
-----------------------------
- cargo check --bin boru --features gui
    → still fails, but ONLY on sibling-owned errors:
      * FS-11 (BLOCKED card): OutboundState, TransferRecord, TransferStateStore,
        ProjectionUpdate, transfer_store, outbound_*, MAX_OUTBOUND_HISTORY,
        PeerDownload, outbound_row, sort_outbound_rows, TransferDirection
      * FS-16 (done but clobbered by FS-15 rewrite): project_validated_remote_shared_file,
        remote_item_status, RemoteItemStatus
      * FS-15/UI-18 in flight: content_area scope, Ellipsis Wrapping, lifetimes,
        u64 deref, duplicated field initializers
    → ZERO errors reference recent_activity / view_recent_download / refresh_dashboard_activity
- Scratch-crate isolation test (module included by #[path] against real boru-core):
    cargo test --test fs12  →  15 passed; 0 failed  (12 module tests + 2 harness + 1 marker)
- cargo test --lib storage::tests::transfer_activity  →  ok (idempotent + survives restart)
- cargo test --lib storage::tests::activity_retention_prunes_old_rows  →  ok
- cargo test --lib storage::tests  →  80 passed; 0 failed

TESTS
-----
recent_activity_view_model.rs has 12 unit tests covering:
- request is never presented as success (vs completion success)
- lifecycle stages map to distinct actions (Queued/Requested/Authorized/Started/
  In progress/Downloaded/Cancelled)
- permission_denied failure → Denied (not generic Failed)
- other failure categories → Failed with bounded detail
- cancelled/paused → Warning
- unknown future event names stay neutral
- newest-first ordering with stable tiebreak
- dedup of replayed event ids
- removed items fall back to safe labels (no '/' or 'hash' in Debug output)
- enrichment uses resolved labels when available
- byte counts only from allowed payload keys
- card subset bounded to MAX_RECENT_ACTIVITY_ROWS
- status labels are short accessible text

RUNTIME / MCP EVIDENCE
----------------------
None possible this run: the binary cannot be built while sibling
workers (FS-11 blocked, FS-13/14/15/17 running) hold the shared tree in a
non-compiling state. This mirrors FS-16's accepted status ("Example blocked by
concurrent-worker code outside FS-16 scope"). FS-23 is the integration card
that will exercise the card against real peers via MCP.

VISUAL EVIDENCE
---------------
None this run (see above). The card uses existing design tokens
(design_tokens::card_style, text_muted/primary_soft, Icon chevron, SPACE_* grid)
matching FS-08's shell and FS-07's primitives.

SECURITY / PRIVACY IMPACT
-------------------------
- Rows never render raw public keys, content hashes, or local paths.
- Peer labels come from the caller's resolved friend/name cache, never from an
  untrusted display field.
- Failure details are limited to the closed error_category taxonomy.
- Payload bytes come only from sanitized keys (bytes_transferred/total_bytes).
- No new storage of sensitive fields; the card reads the existing
  privacy-filtered projection.

KNOWN LIMITATIONS
-----------------
- Card wiring (app.rs/main.rs) is uncommitted; it lives in the shared working
  tree and may be clobbered by sibling rewrites before the serialization pass —
  re-apply per the WIRING section above.
- The "View full activity log" link targets DashboardTab::ActivityLog; the
  by-others/outbound filter on the Activity Log tab itself is FS-17's scope.
- Visual verification deferred to FS-23 (integration) once the example builds.

FOLLOW-UPS
----------
- Serialization pass: commit the app.rs/main.rs FS-12 wiring once siblings land.
- FS-22 (test coverage) should add the recent_activity_view_model tests to the
  main suite (they currently run via scratch-crate isolation because the
  example crate is blocked).

COMMIT
------
bdf7a0a5 feat(FS-12): recent download activity view model for dashboard card
