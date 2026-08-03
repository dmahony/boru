FS-18 "Finish search, filtering, sorting, and tab state retention" — implementation handoff
============================================================================================

CARD
----
FS-18 Finish search, filtering, sorting, and tab state retention (t_50e26e9a)
Phase E - Quality and integration | Depends: FS-09..FS-17 | Gate: Quality gate

STATUS
------
MODULE COMMITTED (see COMMIT below). Shared-file wiring (app.rs / main.rs) is
present in the working tree and fully compiling + tested (cargo check 0 errors,
764 tests pass), but left UNCOMMITTED because app.rs/main.rs are shared with
concurrent sibling workers (FS-09/10/11/12/13/14/15/16/17) whose cumulative
wiring is also in the tree awaiting the serialization pass — exactly the FS-12
precedent (docs/fs-12-implementation-handoff.md). The serialization pass can
commit the shared files as-is; the tree is green. If the tree is clobbered
before then, re-apply the wiring anchors below (each is located by symbol, not
line number).

SUMMARY
-------
FS-18 completes the dashboard's search/filter/sort behaviour:

- Search is GLOBAL (one query preserved across tabs) and each tab interprets it
  against its own relevant fields via a shared, unit-tested normalized matcher:
  Shared by Me = display name + recipient display labels + recipient peer ids
  (short prefix matches via substring); Downloading = display name + peer label
  + short peer id; Downloaded = display name + source peer label; Shared with Me
  = display name + peer label + short peer id; Activity Log = existing peer/file/
  action matching (FS-17). Normalization is trim + Unicode lowercase; empty
  query matches everything; in-memory only (no debounce).
- Sort controls (keyboard-accessible button chips, active key shows ↑/↓) were
  added exactly where the card requires: Shared by Me = date shared / name /
  size / downloads; Downloaded = completed time / name / size; Activity Log =
  time / status. Every sort is deterministic: stable row-id tie-break on every
  key, Unicode-aware name key, Option<u64> (None < Some) for size/downloads,
  status rank for Activity outcome, and "click same key again toggles direction,
  new key switches to its default direction".
- State retention: active tab, query, and all three per-tab sorts live on the
  App screen state (dashboard_active_tab / dashboard_search_input /
  dashboard_shared_by_me_sort / dashboard_downloaded_sort /
  dashboard_activity_sort), so they survive in-session navigation away and back
  (existing screen-state pattern; no persistence to disk required by the card).
- Clear search is one action and keyboard accessible: a real × button in the
  header search row (visible whenever the query is non-empty) and the Escape key
  while on the File Sharing screen (handled by the existing Shortcut::Escape
  branch, which takes precedence over the chat-composer clear there). Both also
  close any half-open Shared by Me interactions (mirrors DashboardSearchChanged).
- Filtering never mutates authoritative data: Shared by Me filters+sorts a clone
  into a stable view-slice field (`dashboard_shared_by_me_filter`, rebuilt by
  `refresh_shared_by_me_filter` on query/sort/rows changes); Downloaded and
  Activity filter+sorts borrows/clones of their authoritative buffers; Downloading
  filters projected clones. Summary cards (Sharing Summary, Recent Download
  Activity, Peers Downloading placeholders) are computed from storage/unfiltered
  data, so counts stay authoritative and active-transfer state is never hidden.
- Shared with Me tab is now actually reachable: it was falling through to the
  Shared by Me layout; FS-18 wires the FS-16 `view_shared_with_me` into its own
  full-content branch (needed so the global search has a real tab to apply to).
  This is the only non-search/sort wiring change and is strictly the FS-16
  dispatch the card's "complete the screen's search/filter/sort behaviour"
  requires.

CHANGED FILES (verified paths)
------------------------------
- examples/iced_chat/dashboard_filters.rs  (NEW — committed; 763 lines incl.
  16 unit tests: normalize, query_matches, SharedByMeSort/DownloadedSort/
  ActivitySort + on_key_clicked + apply/apply_ref, deterministic tie-breaks,
  long/Unicode names, ref-sort parity with owned sort)
- docs/fs-18-implementation-handoff.md      (NEW — this file)
- examples/iced_chat/main.rs                (working tree: `mod dashboard_filters;`
  after `mod dashboard_view_model;`)
- examples/iced_chat/app.rs                 (working tree — wiring anchors below)

WIRING ANCHORS (all in examples/iced_chat/app.rs unless noted; locate by symbol)
-------------------------------------------------------------------------------
1. main.rs: after `mod dashboard_view_model;` add `mod dashboard_filters;`
   (already added in working tree).
2. State fields (near `dashboard_search_input`):
   - dashboard_shared_by_me_sort: crate::dashboard_filters::SharedByMeSort
   - dashboard_downloaded_sort: crate::dashboard_filters::DownloadedSort
   - dashboard_activity_sort: crate::dashboard_filters::ActivitySort
   - dashboard_shared_by_me_filter: Vec<crate::shared_by_me_table::SharedByMeRow>
   Constructor init: the three sorts `::default()`, filter `Vec::new()`.
3. Messages + name()/log_variant debug arms (near DashboardSearchChanged):
   - DashboardSearchCleared
   - DashboardSharedByMeSortClicked(SharedByMeSortKey)
   - DashboardDownloadedSortClicked(DownloadedSortKey)
   - DashboardActivitySortClicked(ActivitySortKey)
4. update() handlers (near the DashboardSearchChanged arm):
   - DashboardSearchChanged: after clearing shared_by_me_ui, call
     self.refresh_shared_by_me_filter() before refresh_dashboard_activity()
   - DashboardSearchCleared: clear input + shared_by_me_ui + refresh filter
   - DashboardSharedByMeSortClicked: sort = sort.on_key_clicked(key); refresh
   - DashboardDownloadedSortClicked / DashboardActivitySortClicked: toggle sort
5. Escape handling (Shortcut::Escape chain): insert before the composer-text
   clear:
     else if matches!(self.screen, Screen::FileSharing)
         && !self.dashboard_search_input.is_empty() {
       self.dashboard_search_input.clear();
       self.shared_by_me_ui.clear();
       self.refresh_shared_by_me_filter();
     }
6. refresh_shared_by_me_filter(&mut self): private method after
   refresh_shared_by_me — filters self.shared_by_me_rows by query against
   display_name + recipient.label + recipient.id, applies
   self.dashboard_shared_by_me_sort, stores into dashboard_shared_by_me_filter.
   Also called from SharedByMeLoaded.
7. Header search row (view_file_sharing): search_input keeps on_input
   DashboardSearchChanged; add clear_search_button (Icon::Close, on_press
   DashboardSearchCleared, shown only when input non-empty) pushed after
   search_input in search_row.
8. view_file_sharing Shared by Me section: replace local filtering with
   `let visible_rows = &self.dashboard_shared_by_me_filter;`, render a
   dashboard_sort_chip row (keys from SharedByMeSortKey::ALL, message
   DashboardSharedByMeSortClicked) above the card, and when the query is
   non-empty and visible_rows is empty show empty_state(Icon::Search,
   "No matching files.", "Try a different search term.") instead of the card's
   "haven't shared any files yet" copy.
9. view_file_sharing dispatch: after the Downloading early-return add:
     if active_tab == DashboardTab::SharedWithMe {
         return scrollable(self.view_shared_with_me())...into();
     }
10. view_downloaded: filtered borrows (Vec<&CompletedDownloadItem>) with
    query_matches against display_name + source_peer, then
    self.dashboard_downloaded_sort.apply_ref(&mut filtered); add a sort chip
    row (DownloadedSortKey::ALL) between header_row and the table header.
11. view_activity_log: make `filtered` mutable, call
    self.dashboard_activity_sort.apply(&mut filtered) after filter_activity_log
    and before paginate; add a sort chip row (ActivitySortKey::ALL) after the
    chips row.
12. view_downloading: after sort_incoming_rows, retain rows matching
    query_matches against display_name + resolved peer label + peer_id.
13. view_shared_with_me: extend the existing search filter to query_matches
    against display_name + peer_label + peer.fmt_short().
14. Free helper dashboard_sort_chip (near dashboard_card): renders a
    keyboard-focusable button chip; active chips show ↑/↓.

DESIGN/ARCHITECTURE DECISIONS
-----------------------------
- Global query, per-tab interpretation (card's preferred behaviour): one
  dashboard_search_input; each tab builds its own haystacks from its relevant
  fields. Documented in dashboard_filters.rs module doc + this handoff; tested
  via query_matches unit tests (case-insensitive Unicode substring).
- Pure logic module (dashboard_filters.rs) keeps Iced out of search/sort logic:
  normalization, matching, sort enums/state machines, and comparators are all
  unit-testable without a widget tree. Sort state is tiny (Copy structs) so it
  lives directly on App like dashboard_active_tab.
- Filtering/sorting the Shared by Me table uses a stable view-slice field
  rebuilt in update() (refresh_shared_by_me_filter) instead of per-frame local
  clones: avoids re-filtering on every render AND keeps the borrowed lifetime
  clean for view_shared_by_me_card (the card borrows &self-held rows). The
  authoritative shared_by_me_rows buffer is never mutated.
- Downloaded/Activity keep borrows (apply_ref) or clone-then-sort on the
  filtered subset; the authoritative history/activity buffers are never sorted
  or truncated. Deterministic tie-break = stable row id always ascending, so
  repeated renders and duplicate display names produce identical order.
- No debounce: all filtering is in-memory over already-projected rows
  (card's instruction).
- Clear × button only when text present (declutter); Escape on File Sharing
  screen clears first (before the composer-clear branch) so the keyboard flow
  is one key.
- Shared with Me dispatch wiring is the minimal FS-16 completion required for
  search to apply there; no other FS-16 behaviour changed.
- No in-app file browser: native OS file picker untouched.

COMMANDS RUN (exact + result)
-----------------------------
- cargo check --example boru --features gui
  → exit 0, no errors (208 pre-existing warnings, unchanged)
- cargo test --example boru --features gui dashboard_filters
  → test result: ok. 16 passed; 0 failed (normalization, matching, sort
    tie-breaks, ref-sort parity, long/Unicode names, empty query)
- cargo test --example boru --features gui
  → test result: ok. 764 passed; 0 failed (full example suite green)

TESTS
-----
- 16 new unit tests in dashboard_filters.rs:
  - normalize: trim, Unicode lowercase (Grüße, İSTANBUL), empty/whitespace
  - empty_query_matches_everything
  - matching_is_case_insensitive_unicode_substring
  - long_and_unicode_names_match_fully
  - shared_by_me default is date-shared descending; on_key_clicked toggles
    direction on same key, switches key + default direction otherwise
  - shared_by_me sort determinism incl. duplicate names tie-broken by id,
    and Option<u64> (size/downloads) ordering
  - downloaded time/name/size sorts deterministic
  - downloaded_ref_sort_matches_owned_sort (apply_ref parity)
  - activity time newest-first with id tiebreak; status rank ordering
  - state-restoration semantics covered by the Copy structs + Default + toggle
    round-trips (query/tab/sort live on App fields, per acceptance criterion)
- Full suite: 764 passed / 0 failed.

RUNTIME/MCP EVIDENCE
--------------------
- No MCP session was available for this run; runtime/interactive verification
  (filtered-state screenshot, live-update stability under a running transfer,
  route-change round-trip in the UI) is deferred to the integration task
  (FS-23 precedent from FS-12). Static guarantees: filters operate on clones/
  borrows of projected rows, never on the live inbound store, so progress
  updates keep flowing while a query is active (the Downloading filter retain()
  happens on the projected Vec after sort_incoming_rows; the underlying
  inbound_active map and TransferStateStore are untouched).

VISUAL EVIDENCE
---------------
- None captured this run (headless). The sort chip rows, × clear button, and
  search-specific empty states follow the existing dashboard card/chip/badge
  design tokens (dashboard_card, chips, empty_state) and compile into the same
  widget tree; deferred to FS-23 for screenshots.

SECURITY/PRIVACY IMPACT
-----------------------
- None new. Matching uses only fields already resolved for display (display
  names, peer labels, and public-key ids as substring haystacks — the public
  key is not secret, and no raw key/hash/path is rendered; the short-id
  haystacks are the same information fmt_short already shows). No new storage,
  no new network, no data exfiltration paths, no picker replacement.

KNOWN LIMITATIONS
-----------------
- app.rs/main.rs wiring uncommitted pending the serialization pass (shared
  with 9 sibling tasks' cumulative wiring; tree is green and ready to commit).
- "Downloads" column on Shared by Me always renders "—" today (FS-09 finding:
  the durable projection has no download count yet); the sort key exists and is
  deterministic (None < Some) but is inert until FS-01/backend records counts.
- Recent Download Activity / Peers Downloading summary cards are NOT filtered
  by the header query by design (authoritative counts) — if the review prefers
  those to also filter, that is a deliberate product change, not a bug.
- Visual/runtime evidence deferred to FS-23 integration verification.

FOLLOW-UPS
----------
- Serialization pass: commit shared-tree wiring (app.rs/main.rs) — the current
  tree is green; no re-application needed unless clobbered (anchors above).
- FS-23 integration: capture filtered-state screenshot, live-update search
  stability with a running transfer, in-session route-change round-trip,
  duplicate-display-name sort check.
- When FS-01 exposes real download counts, fill the Shared by Me "Downloads"
  column; the sort key already handles Some values.
- Optional: consider an explicit "Showing X of Y" hint in the card badge when a
  query is active (count badge currently shows the filtered count).

COMMIT
------
feat(FS-18): dashboard search normalization, per-tab sort controls, and tests
(Hash recorded in kanban metadata; module dashboard_filters.rs + this handoff.)
