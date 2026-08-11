FS-13 "Sharing Summary" card — implementation handoff
======================================================

Status: MODULE COMMITTED (this commit); WIRING PRESENT IN WORKING TREE, uncommitted
because app.rs/main.rs/storage.rs are shared with concurrent sibling workers
(FS-09/15/16/17 etc.) whose half-done code currently blocks the example build and
whose own uncommitted storage additions share the same files. Apply the wiring
below in the serialization pass (or when siblings land), then run the verification
steps at the end.

CARD
----
FS-13 Implement the Sharing Summary card (t_18676d6c)

SUMMARY
-------
The lower-right "Sharing Summary" card is implemented as a self-contained module
`examples/iced_chat/sharing_summary.rs`. It renders four all-time metrics — Files
shared, Total downloads, Active downloads, Peers you've shared with — in a clean
two-column layout (large values, small labels) matching the FS-02 mockup, with a
truthful "All time" scope label and an explicit loading/unknown state that renders
em dashes instead of a premature zero. All numbers are derived from durable
SQLite rows via a pure projection function (`project_sharing_summary`), never from
rendered rows and never by scanning the filesystem.

Metric definitions (documented in the module and locked by unit tests):
- Files shared        = number of distinct retained `shared_files` rows for the
                        local profile. One row is one shared item; folders are not
                        a shareable unit in this version (the native OS picker
                        selects files, each indexed file is its own row). Stopping
                        a share deletes the row, so the count is truthful for
                        retained records.
- Total downloads     = `downloads` rows in a terminal completed state
                        (`complete` / `completed`) only. Failed, cancelled, and
                        version-mismatch rows are NOT counted.
- Active downloads    = `downloads` rows in a non-terminal state (queued,
                        resolving_peer, requesting_permission, downloading,
                        verifying, paused). Live count — changes as the download
                        state machine transitions rows.
- Peers you've shared with = distinct `grantee_user_id` values in
                        `shared_file_permissions` where `grantor_user_id` is the
                        local profile. Unique peers identified by their
                        hex-encoded public key; a peer granted access to several
                        files counts once.

COMMITTED ARTIFACT
------------------
examples/iced_chat/sharing_summary.rs  (new module, projection + view + 6 unit tests)

STORAGE HELPERS REQUIRED (present in the working tree, uncommitted)
------------------------------------------------------------------
The projection reads three authoritative queries. Two of them did not exist at
HEAD and were added to `src/storage.rs` in the working tree; they must be
committed in the serialization pass together with the wiring:
- `Storage::list_downloads()`            — every download row, any state, oldest first
- `Storage::list_shared_peer_ids(grantor_user_id)` — DISTINCT grantees, deterministic order
(plus their storage unit tests: list_downloads_returns_all_states_oldest_first,
list_shared_peer_ids_is_distinct_and_deterministic,
summary_projection_counts_survive_restart — 3/3 passing).

The third query already exists at HEAD: `Storage::list_shared_files(profile, false)`.

WIRING TO APPLY (in the serialization pass)
-------------------------------------------
1. examples/iced_chat/main.rs — declare the module (already added in the working
   tree, may be clobbered):
     mod sharing_summary;

2. examples/iced_chat/app.rs — the following FS-13 pieces are present in the
   working tree; re-locate by symbol if the tree changed:
   - IcedChat field:
       dashboard_sharing_summary: Option<crate::sharing_summary::SharingSummary>
   - init:                 dashboard_sharing_summary: None
   - message:              DashboardSharingSummaryLoaded(Option<crate::sharing_summary::SharingSummary>)
   - name() arm:           AppMessage::DashboardSharingSummaryLoaded(_) => "DashboardSharingSummaryLoaded"
   - update() arm:         AppMessage::DashboardSharingSummaryLoaded(summary) => { self.dashboard_sharing_summary = summary; ... }
   - refresh task:         fn refresh_sharing_summary(&self) -> iced::Task<AppMessage>
     (spawn_blocking; maps the JoinHandle result with `.ok().flatten()` so the
     message payload is Option<SharingSummary>, not Result)
   - card view:            fn view_sharing_summary_card(&self) -> iced::Element<'_, AppMessage>
   - refresh call sites:   self.refresh_sharing_summary() on OpenFileSharing and on
                           DashboardTabSelected(DashboardTab::SharedByMe)
   - layout: replace the "Sharing Summary" placeholder_card in BOTH the wide and
     narrow branches with:
         let sharing_summary_section = self.view_sharing_summary_card();

VERIFICATION STEPS (run after wiring is committed)
--------------------------------------------------
1. cargo check --features gui --bin boru        # expect 0 errors
2. cargo test --features gui --bin boru -- sharing_summary   # 6/6 pass
3. cargo test --lib --features gui -- storage::tests::list_downloads \
     storage::tests::list_shared_peer_ids \
     storage::tests::summary_projection_counts_survive_restart     # 3/3 pass
4. Visual: open File Sharing → Shared by Me; confirm the lower-right card shows
   the four metrics with an "All time" label, em dashes before the first load
   completes, and live updates after a transfer completes.

KNOWN LIMITATION
----------------
This commit intentionally contains only the self-contained module. The app.rs /
main.rs wiring and the two storage helpers live in the shared working tree and
must be committed by the serialization pass (they are already written, tested, and
verified to compile — the example build is currently blocked only by sibling
in-flight code, not by FS-13 code).
