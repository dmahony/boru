# FS-20 — Security and privacy hardening pass

## CARD / STATUS

- Card: FS-20 — Run a security and privacy hardening pass
- Status: Review complete; high-severity findings fixed; residual risks documented.
- Repository: `/home/dan/iroh-gossip-chat` (boru-core)
- Scope: File Sharing dashboard (FS-09..FS-17 projections) and the backend
  authorization, descriptor, persistence, and telemetry paths they consume.
- Baseline: `docs/fs-01-file-sharing-lifecycle.md` (commit `135b1995`),
  `docs/security-model.md`, `docs/privacy-model.md`.

## SUMMARY

The dashboard's projections are privacy-safe and do not weaken Boru's trust
model by themselves: peer identity for "who is downloading" comes from the
authenticated QUIC connection (`endpoint_id`), remote catalogue entries are
signature- and metadata-validated before they are rendered, completed-download
rows carry no raw local paths into the UI, and the durable activity log stores
only an allowlisted payload. The review found two high-severity backend issues
and one medium defense-in-depth gap, fixed them, and documented the remaining
protocol-level limitations honestly.

### High-severity findings fixed

1. **Permission expiry was not enforced at request time.** The file-access
   handler and catalogue visibility loops called `list_permissions_for_grantee`,
   which returns grants regardless of `expires_at_ms`, and marked any `read`
   grant as explicitly granted — including an expired one. When a file had an
   active grant to *another* peer (selected-peers mode), a requester whose own
   grant had expired was still authorized. Fixed by treating expired grants as
   inert in every in-memory authorization loop (storage, catalogue handler,
   file-access handler) and adding regression tests for read-grant expiry,
   active-grant happy path, and expired-deny lapse.

2. **Dashboard download initiation bypassed backend authorization.** The
   Shared With Me / peer-catalogue Download button routed through the legacy
   `download_blob_to_file` path with no request-time check: no verified
   catalogue precondition, no durable download row, no signed descriptor.
   The UI's `can_download` state was the only gate. Fixed by:
   - persisting the validated, signed catalogue on `BrowsePeerCatalogue`
     (so the backend has an authoritative snapshot to re-check);
   - adding a backend gate `download_initiation::validate_download_request`
     that performs the same catalogue-verified + file-metadata preconditions
     as `initiate_download` without creating a row;
   - calling that gate from `AppMessage::RequestFileDownload` and refusing
     the transfer with the backend's reason when it fails.
   The dashboard no longer starts a transfer the backend would refuse.

### Medium finding fixed

3. **Remote-controlled filename joined directly into the download path.**
   `RequestFileDownload` built `dl_dir.join(&display_name)`. Catalogue
   validation already rejects separators and control characters, but the
   write site now routes through the shared `safe_destination_path` helper
   (strip separators, reject traversal, dedupe collisions, hash fallback
   stem) as defense in depth.

## SECURITY REVIEW — ACCEPTANCE CRITERIA

| Criterion | Status | Evidence |
|---|---|---|
| No UI-only authorization enforcement exists | Fixed | `validate_download_request` gate in `RequestFileDownload`; backend preconditions enforced before any transfer |
| Peer identity is authenticated | Verified | FS-11 outbound panel uses provider `ClientConnected.endpoint_id` (authenticated QUIC peer), never a display string |
| Local paths/secrets are not leaked | Verified | Activity payload allowlist; completed-download rows carry destination for Open/Reveal but UI renders labels only |
| Remote metadata is safely handled | Verified + hardened | `RemoteSharedFile::validate` rejects separators/control/length/MIME; write site now also uses `safe_destination_path` |
| Replay/race cases are tested | Verified | Nonce store tests; projection dedup/sequence tests; new expiry regressions |
| Revocation and expiry remain enforced by backend | Fixed | Expired grants inert in all loops; request-time re-check still the backend's `FileAccessHandler` for the authorized path |

## FINDINGS IN DETAIL

### F1 — Expired permission grants still authorized downloads (HIGH)

**Where.** `src/file_access_handler.rs` `check_permission` (two loops),
`src/catalogue_handler.rs` visibility loop, `src/storage.rs`
`catalogue_entries_for_peer`. All four called `list_permissions_for_grantee`
(no expiry filter) and treated any `read` grant as active.

**Impact.** If file F had an active read grant to peer A and an *expired* read
grant to peer B, B was authorized: `count_read_grants_for_file` saw A's active
grant (`has_any_read_grants == true` → selected-peers mode) and the loop saw
B's expired grant (`explicitly_granted == true`) → not denied. The owner's
intended expiry did not revoke access.

**Fix.** Added `SharedFilePermission::is_active_at(now_ms)` and skip expired
grants in all four authorization loops. Deny grants with an expiry also lapse,
consistent with the SQL-level `check_permission` filter.

**Tests.** `expired_read_grant_does_not_authorize`,
`active_read_grant_authorizes`, `expired_deny_grant_does_not_deny_friend`,
`permission_is_active_at_respects_expiry_boundary`,
`catalogue_entries_for_peer_hides_file_with_only_expired_grants`,
`catalogue_entries_for_peer_still_lists_active_grant`.

### F2 — Dashboard Download bypassed backend request-time authorization (HIGH)

**Where.** `examples/iced_chat/app.rs` `AppMessage::RequestFileDownload`
→ `download_blob_to_file` (legacy), and `BrowsePeerCatalogue` which kept the
fetched catalogue only in memory.

**Impact.** The dashboard could start a download for a file the backend no
longer advertised in a verified catalogue (revoked/removed/stale), and the
transfer never consulted the backend's authorization state. The acceptance
criterion "no UI-only authorization enforcement" failed.

**Fix.**
- `BrowsePeerCatalogue` now persists the validated signed catalogue via
  `catalogue_client::process_and_store_remote_catalogue` (storage available).
- New `download_initiation::validate_download_request(storage, hash, peer)`
  runs the catalogue-verified + file-metadata checks without creating a row.
- `RequestFileDownload` calls it before starting the transfer; on failure the
  row is marked `Failed` with the backend's reason and no transfer starts.

**Tests.** `validate_download_request_passes_for_verified_catalogue`,
`validate_download_request_rejects_missing_catalogue`,
`validate_download_request_rejects_file_not_in_catalogue`,
`validate_download_request_does_not_create_download_row`.

### F3 — Remote filename used directly in the download path (MEDIUM)

**Where.** `RequestFileDownload` save path construction.

**Fix.** `safe_destination_path(&dl_dir, &display_name, &content_hash)`
(shared helper, already unit-tested). Rejects traversal, strips separators,
dedupes existing files, falls back to the content-hash stem.

## PATH / IDENTITY / LOG LEAKAGE AUDIT

| Surface | Verdict |
|---|---|
| Outbound "who is downloading" peer | Authenticated `endpoint_id` from provider events; label resolved via friends store. No remote-supplied display string trusted. |
| `RemoteSharedFile` wire fields | No path, DB row id, blob ticket, or upload secret. Validation rejects separators, control/format chars, invalid MIME, oversized fields, future timestamps. |
| Activity log payload | `sanitize_activity_payload` allowlist: totals, counters, category, direction. No filenames, hashes, paths, or peer keys. |
| Completed-download rows | Store `destination_path` locally for Open/Reveal; dashboard renders display name/size/status only. `local_file_state` returns Verified/Warning/Missing. |
| Download save path | Now via `safe_destination_path`. |
| Local shared-file `source_path` | Local-only (`file_objects.source_path`), never a wire field; `project_local_shared_file` test asserts no path leak into the projection. |
| Logs | Transfer telemetry excludes filenames/paths/hashes/peer ids. Access diagnostics use short peer ids and a shared-file-id prefix. |

## REPLAY / RACE REVIEW

- Descriptor nonces are single-use (`NonceStore::check_and_mark`, expiry
  eviction). Covered by existing nonce tests.
- Transfer projection dedups by event id and rejects stale sequences;
  post-terminal events are ignored. Covered by FS-05 tests.
- Activity log dedups by event id (`INSERT OR IGNORE`) and the projection
  dedups again by `event_id`.
- Cancel path (`cancel_inbound_transfer`) publishes a `Cancelled` projection
  event and, when a durable row maps to the same content hash, calls
  `DownloadManager::cancel_download`. Terminal-state guards in
  `handle_download_progress` prevent late progress from flipping a completed
  row back to Active.

## DESIGN / ARCHITECTURE DECISIONS

- **Backend remains authoritative.** The UI gate is a call into the backend
  (`validate_download_request`), not a UI state. The gate is validation-only
  (no durable row) so a refused transfer leaves no orphan state.
- **Expired grants are inert, not sticky.** Consistent with the SQL helpers
  (`check_permission`, `count_read_grants_for_file`,
  `has_active_permissions_for_file`) which already filter by expiry.
- **Legacy transfer mechanics preserved.** The authorized durable worker
  (`request_and_transfer_blob` + `verify_install_and_complete`) remains the
  descriptor-bearing path; this pass did not rewire the GUI's byte transfer to
  it, which is a separate integration follow-up (see limitations).
- **Native OS file picker preserved.** No in-app file browser was introduced.
- **Sibling compile fixes (FS-19 scope, behavior-preserving).** The serialization
  commit 2702f0cc landed `dashboard_connectivity_notice` and
  `ConnectivityNotice` with compile errors. FS-20 fixed them minimally:
  `ConnectivityNotice` never borrowed (String + Copy enum + owned Option), so
  its phantom lifetime was removed and `build` returns `Element<'static>`; the
  icon color is selected as a non-capturing token fn pointer per severity.
  Rendering output is identical.

## COMMANDS RUN

- `cargo check --lib` → PASS (3 pre-existing warnings).
- `cargo test --lib -- expired_ catalogue_entries_for_peer permission_is_active_at`
  → 13 passed / 0 failed (new expiry regressions + existing).
- `cargo test --lib -- download_initiation` → 19 passed / 0 failed (gate tests).
- `cargo test --lib -- file_access_handler catalogue_handler`
  → 72 passed / 0 failed (expiry regressions + existing suites).
- `cargo test --lib` (full suite) → see COMMIT section for the final result.
- `cargo check --example boru --features gui` → PASS after fixing two
  pre-existing FS-19 sibling compile errors (`dashboard_connectivity_notice`
  missing lifetime; `ConnectivityNotice` phantom lifetime + capturing
  `color_fn` closure). Those files were committed by the FS-19/UI-19
  serialization pass (2702f0cc) with compile errors; FS-20 fixed them so the
  dashboard binary can boot for security verification. Both fixes are
  behavior-preserving (see DESIGN DECISIONS).

## TESTS

New tests (all passing):
- `src/file_access_handler.rs`: 3 expiry regressions.
- `src/storage.rs`: `permission_is_active_at_respects_expiry_boundary`,
  `catalogue_entries_for_peer_hides_file_with_only_expired_grants`,
  `catalogue_entries_for_peer_still_lists_active_grant`.
- `src/download_initiation.rs`: 4 `validate_download_request` gate tests.

## KNOWN LIMITATIONS / RESIDUAL RISKS (honest)

1. **The GUI byte-transfer path is still the legacy hash download.** The
   dashboard now enforces the backend's verified-catalogue precondition and
   metadata validity, but it does not yet issue a `SignedDownloadDescriptor`
   for GUI-initiated transfers. Active-transfer remote revocation is not
   possible with the current protocol (FS-01 §5 documents this: no live revoke
   signal; an already-issued descriptor can finish). Wiring the dashboard to
   `request_and_transfer_blob` + `verify_install_and_complete` is a follow-up
   (integration), not silently claimed as done.
2. **Blob provider serves by hash to any authenticated peer.** The stock
   iroh-blobs `BlobsProtocol` has no per-request allowlist hook; catalogue
   disclosure is the effective capability gate. The dashboard does not weaken
   this, but revocation after hash disclosure is inherently limited. A future
   provider-side authorization hook would be required for full request-time
   enforcement.
3. **`view_shared_with_me` derives status from catalogue presence + peer
   online, not from live permission state.** `Expired`/`Revoked` status
   variants exist but the caller passes `false` for both; the honest signal is
   the backend gate at download time. Documented, not faked.
4. **Permission expiry is checked at request time, not continuously.** A grant
   that expires mid-transfer is not interrupted; the descriptor path enforces
   descriptor TTL, not the grant expiry. Acceptable for the documented model.
5. **Sibling compile debt fixed in this pass.** The example build was broken
   at HEAD by FS-19 connectivity-notice code (`dashboard_connectivity_notice`,
   `ConnectivityNotice`) that landed with lifetime/closure compile errors in the
   serialization commit. FS-20 fixed both minimally and the example now
   compiles; the fixes are behavior-preserving and outside the security review's
   core findings (documented in DESIGN DECISIONS).

## FOLLOW-UPS

- Wire dashboard download initiation to `request_and_transfer_blob` +
  `verify_install_and_complete` (signed descriptor, expiry, nonce,
  hash-verified install) — FS-23 integration scope.
- Consider a provider-side blob authorization hook so revocation after hash
  disclosure is enforceable at the transfer layer.
- Re-run the example build after sibling FS-19/UI-19 work lands.

## CHANGED FILES (verified paths)

- `src/storage.rs` — `SharedFilePermission::is_active_at`, expiry filter in
  `catalogue_entries_for_peer`, tests.
- `src/file_access_handler.rs` — expiry filter in both `check_permission`
  loops, 3 regression tests.
- `src/catalogue_handler.rs` — expiry filter in visibility loop.
- `src/download_initiation.rs` — `validate_download_request` gate + shared
  `find_verified_catalogue_entry` + 4 tests.
- `examples/iced_chat/app.rs` — persist validated catalogue on browse; gate
  `RequestFileDownload`; `safe_destination_path` for save path; fix FS-19
  `dashboard_connectivity_notice` lifetime so the example compiles.
- `examples/iced_chat/ui_components.rs` — remove phantom lifetime from
  `ConnectivityNotice` (Element<'static>); non-capturing icon color fn.
- `docs/fs-20-security-review.md` — this report.

## COMMIT

`2f64a1d1` — review(FS-20): security hardening — expiry enforcement, download gate, safe path, compile fixes
