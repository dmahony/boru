# Boru integration-test regression matrix (BORU-TEST-011)

This document is the output of **BORU-ARCH-36 / BORU-TEST-011** (audit overlapping
integration tests). It groups the `tests/` integration suite by capability, records
the invariant each file guards, flags the *unique* coverage that must never be lost
(fault injection, directionality, restart), and records the consolidation work that
was done (and still remains) so redundancy does not silently grow back.

Scope note: Boru has *thousands* of individual assertions across ~140 test files.
The matrix below is a **file-level** audit — each row names the file, the number of
test cases, and the invariant(s) it owns. This is the right granularity for
deciding what is redundant; function-level tables for specific components live
next to those components (e.g. `tests/test_download_initiation_integration.rs`)
and are referenced here.

Legend for the **unique-coverage** column:

- `D` — directionality / bidirectional-order coverage (a flow that is order- or
  initiator-sensitive and must be exercised from *both* sides or in both starts).
- `R` — restart / crash / reopen durability (state must survive process death).
- `F` — fault injection / hostile or malformed input (deliberate error paths). These
  are the cases PDF BORU-TEST-011 says **must not be removed**.

---

## 1. Discovery

| File | Tests | Invariant / unique coverage |
|------|-------|-----------------------------|
| `test_discovery_startup.rs` | 4 | Startup subscribes to the discovery topic without a conversation; canonical mainnet topic `v1`; topic derivation/classification guards; prompt shutdown. |
| `test_discovery_two_node.rs` | 5 | Two-node advertises → dial candidate → control presence, without a lobby chat; presence goes stale after peer disappears; `discovered-but-not-direct-topic-ready` state machine gate. `R` |
| `test_discovery_restart.rs` | 5 | Restarted peer rejoins discovery; offline-then-reconnect; control-presence restore without manual action; restart B while A open triggers auto-reconnect; reconcile restores only existing direct topics. `R` |
| `test_discovery_e2e_matrix.rs` | 9 | **Canonical ordering matrix**: A-first vs B-first (`D`), both-offline-then-reconnect, LAN-direct vs relay-required paths, direct-open in 0/1/2 directions (`D`), multiple conversations + discovery. **Do not lose the `D` scenarios.** |
| `test_discovery_dm_isolation.rs` | 2 | Direct messages use only the direct topic; discovery vs direct topics are domain-separated. |
| `test_discovery_group_isolation.rs` | 2 | Group messages stay on the group topic while discovery runs; group vs discovery domain separation. |
| `test_discovery_ui_isolation.rs` | 5 | Malformed / valid discovery traffic produces no UI state; forwarding drops discovery-topic events; invalid payload rejection; control-plane envelope routing; malformed frame dropped but valid still processed. `F` |

Discovery files look superficially similar (same two-peer scaffolding) but each
guards a **different invariant** (startup, two-node state machine, restart, ordering
matrix, per-domain isolation). They are **not** redundant; they were partly
de-duplicated by BORU-TEST-010 (shared `support::peers` / `support::net`).

## 2. Messaging

| File | Tests | Invariant / unique coverage |
|------|-------|-----------------------------|
| `test_message_lifecycle.rs` | 39 | Delivery state machine: full Queued→…→Seen transitions, all failure arcs, restart recovery preserving states/retry-count, atomic reload. `R` |
| `test_message_transfer.rs` | 1 | End-to-end two-peer message transfer (iced-style path). |
| `test_signed_gossip_flow.rs` | 2 | Signed + compressed signed gossip message flow. `F` |

Note: `src/` also carries unit-level delivery-state and `download_initiation`
tests. Integration vs unit duplication here is **intentional layering** (public-API
consumer view vs in-crate unit view), not redundancy.

## 3. Transfer (files / blobs / downloads / outbox)

| File | Tests | Invariant / unique coverage |
|------|-------|-----------------------------|
| `test_download_initiation_integration.rs` | **15** (was 21) | Download initiation preconditions + happy path. **Consolidated in this task (BORU-TEST-011): 5 `DownloadAlreadyExists` conflict-state cases and 3 `FileMetadataInvalid` cases folded into two table-driven tests. No coverage lost (all assertions preserved, incl. the "original row left intact" check).** |
| `test_download_integration.rs` | 10 | Small/large download, retry schedule, pause/resume/descriptor, resume-rejects-hash-change, list-by-state, idempotent pause. |
| `test_download_queue_order.rs` | 12 | FIFO ordering, global/per-peer active caps, back-pressure, starvation avoidance, bounded startup burst. |
| `test_download_recovery.rs` | 6 | Interrupted-state recovery on reopen; verifying-valid-temp installs to completed. `R` |
| `test_interrupted_transfer_harness.rs` | 36 | Crash/interruption durability per transfer state; no duplicate downloads. `R` `F` |
| `test_interruption_restart.rs` | 5 | Pause preserves progress; resume re-resolves peer; fresh permission required; descriptor hash change → version mismatch; expired descriptor keeps paused. `R` |
| `test_transfer_lifecycle_telemetry.rs` | 24 | Full transfer lifecycle + telemetry (monotonic checkpoints, zero-total guard, retry attempt counting, stable error names). |
| `test_outgoing_dm_transaction.rs` | 9 | Outgoing DM atomicity: exact encrypted outbox, idempotent retry, rollback on conflict/encryption/DB failure, sequence survival across restart. `R` `F` |
| `test_outbox_throughput.rs` | 3 | Sequential vs concurrent delivery ordering/latency semantics. `D` |
| `test_instant_file_sharing_required.rs` | 20 | Instant file sharing invariants (offer-before-ingest, idempotent offer, unauthorized reject, no sender path on wire, size+BLAKE3 verify, early-EOF safety). `F` |
| `test_pause_scenarios.rs` | 15 | Pause/resume/cancel edge cases (idempotent, terminal rejection, temp cleanup, stale-progress reject, independence). |
| `test_normal_downloads.rs` | 1 | Coverage across empty/small/large/imported/referenced/duplicate files in one flow. |
| `test_file_library_integration.rs` | 9 | File library lifecycle (import/reference/associate/dedup, missing source, DB-row-without-bytes). |

## 4. Room / catalogue

| File | Tests | Invariant / unique coverage |
|------|-------|-----------------------------|
| `room_e2e.rs` | 1 | End-to-end room messaging. |
| `test_public_room_directory.rs` | 16 | Public room directory: discoverable/unlisted/private advertising, TTL expiry/re-publication, withdrawal, metadata-change-dedupe, malformed/oversized advertisement rejection. `R` `F` |
| `test_public_lobby_integration.rs` | 8 | Public lobby presence, first-peer-alone (`D`), DHT lookup failure, malformed/stale records, leave stops publication. `F` |
| `test_private_room_dht_discovery.rs` | 10 | Private-room DHT discovery: secret round-trip, namespace isolation, chain of 3, creator-offline, dedupe/filter, idempotent shutdown. |
| `test_private_room_invitation_discovery.rs` | 15 | Invitation-based discovery: offline flow, backend outage recovery, stale+valid, v1 wire compat, determinism. `R` `F` |
| `test_room_invite_v2.rs` | 19 | Room invite v2 parse matrix (prefix/length/version/base32/corruption); debug redaction; no legacy overlap. `F` |
| `test_lobby_migration.rs` | 4 | Legacy lobby → public-room SQLite migration (fallback, clean install, canonical-topic-only). |
| `test_catalogue_harness.rs` | 24 | `CatalogueHarness` visibility/permission rules, cache NotModified/compatability, offline cache, pagination revision change, signature rejection. `R` `F` |
| `test_catalogue_lifecycle_events.rs` | 12 | Catalogue event emission (exactly-one per lifecycle stage, no sensitive payloads, multi-event notice). |
| `test_catalogue_minimal.rs` | (module) | Minimal catalogue harness module (declared, used by other files). |
| `test_malformed_catalogue.rs` | 20 | Malformed-catalogue rejection matrix (garbage, truncated, wrong variant, bad sig, dup ids, empty fields, oversized, dangling refs, recovery). `F` |
| `test_remote_catalogue_integration.rs` | 13 | Remote catalogue: visibility, revision/NotModified, pagination during revision change, offline stale display, dynamic block/revoke. `R` `F` |
| `test_pairing_integration.rs` | 5 | Pairing resolution edges (empty, invalid key, unreachable, round trip, multiple). |

**Catalogue overlap note:** `test_catalogue_harness`, `test_remote_catalogue_integration`
and `test_stable_identities` / `test_peer_lifecycle` all exercise visibility rules,
but through *different harnesses* (localhost `CatalogueHarness` vs real two-peer
transfer vs fixture). Their permission-rule scenarios overlap heavily (friend /
non-friend / blocked / explicit grant / deny). Consolidating the *permission-rule*
matrix into one canonical source is a **candidate follow-up** — do it only by
pulling the shared visibility-rule table into one harness and re-deriving the
others, preserving each file's distinct restart/directionality coverage. **Not done
in this task** because it risks losing restart coverage (`R`) that lives in the
multi-peer harnesses.

## 5. Recovery

| File | Tests | Invariant / unique coverage |
|------|-------|-----------------------------|
| `test_crash_recovery.rs` | 19 | Crash durability of retry/sync records across reopen (states, due-time, no inconsistent mixed state, idempotent replay). `R` |
| `test_sync_after_downtime.rs` | 9 | Sync after downtime: pagination, size truncation, wrong-recipient reject, already-served exclusion, gaps, cursor lifecycle, sync-vs-retry table separation, prune. `R` |
| `test_restart_storm_prevention.rs` | 4 | Restart storm burst limiting / concurrent cap / notify-based cap / all-items-start. `R` |
| `test_reconnect_asymmetric.rs` | 1 | Asymmetric reconnect: messages flow **both directions** after one side restarts. `R` `D` |
| `test_stale_bootstrap.rs` | 5 | Stale bootstrap peers: dedup, memory-lookup seed, room-store roundtrip, stale peer does not block join. |
| `stale_bootstrap.rs` | 1 | Stale bootstrap does not block rejoin (edge). |
| `tunnel_reconnect.rs` | 7 | Tunnel link reconnect with backoff, expiry stops reconnect, enrollment-token one-time/denial, capability-open after enrollment change. `R` `F` |

## 6. Security

| File | Tests | Invariant / unique coverage |
|------|-------|-----------------------------|
| `test_hostile_input.rs` | 41 | Hostile/garbage input rejection (tampered/flipped/empty signatures, spoofed sender, timestamp skew, truncated postcard). `F` — **must not be removed.** |
| `test_malicious_filenames.rs` | 48 | Filename/path safety (traversal, separators, dot-segments, reserved names, control chars, dedup, escape). `F` |
| `test_metadata_security.rs` | 31 | Metadata sanitization (script/HTML/template injection, bidi overrides, oversized name/description, no-OOM). `F` |
| `test_policy_conformance.rs` | 10 | Canonical file-policy admission rules; destination never escapes; hostile-name rejection; case-insensitivity. `F` |
| `test_verify_containment_properties.rs` | 9 | Path-containment properties (errors name offender, canonicalize stays in dir, symlink escape caught, Windows reserved names, UNC). `F` |
| `test_blob_size_enforcement.rs` | 3 | Blob size safety cap enforcement. `F` |
| `test_corrupted_content.rs` | 2 | Corrupted content rejected before install; retry doesn't reuse partial. `F` |
| `test_extensions_metadata.rs` | 3 | Extensions: wire is metadata-only; envelope never touches peer registry. |
| `test_security.rs` | 8 | No secrets in serialized Iced snapshot / failure analysis / journal / classify. |
| `security.rs` | (module) | Security test module (declared). |
| `test_serde_format.rs` | 1 | Conversation serde format stability. |

Security overlaps heavily at the *assertion* level but each file targets a
distinct attack surface (signatures vs filenames vs metadata vs blob-size vs
containment). Most already use table-driven forms internally.

## 7. UI / onboarding

| File | Tests | Invariant / unique coverage |
|------|-------|-----------------------------|
| `test_full_chat_list_flow.rs` | 1 | Full chat-list navigation flow (GUI). |
| `test_iced_chat_flow.rs` | 1 | Iced chat exact flow (GUI). |
| `test_image_iced_gui_flow.rs` | 1 | Image send/download through the GUI (exact). |
| `repro_two_iced_instances.rs` | 2 | Two concurrent GUI instances, different vs same key. |
| `verify_gui_bootstrap.rs` | 1 | GUI bootstrap plumbing. |
| `test_ui_file_sharing_integration.rs` | 11 | Peer-profile UI file sharing (data flow, refresh/stale, collection browse, download-from-profile, pause/resume, permission/version/verify, completed state). |
| `fs22_dashboard_coverage.rs` | 15 | File-sharing dashboard view-model: truthful lifecycle projection, dedupe/order, retention, no-open-when-missing, status precedence, permission expiry. |
| `test_health_view.rs` | 3 | Health view: comparable symmetric dumps, asymmetric failure obvious, probe tasks feed store. `D` |
| `test_onboarding_integration.rs` | 30 | Onboarding state machine (fresh/empty/legacy inference, persistence across save/restart, re-onboarding). `R` |

## 8. Voice / media / other capabilities

| File | Tests | Invariant / unique coverage |
|------|-------|-----------------------------|
| `call_e2e.rs` | 3 | Call lifecycle: complete + busy-reject, no state/connection growth, 75× teardown no leaks. |
| `call_audio_integration.rs` | 2 | Synthetic sine round-trip frame/duration counts. |
| `call_video_integration.rs` | 2 | Video fragment reorder/reassemble/decode; parseable round trip. |
| `call_perf_measurement.rs` | 8 | Voice/video bitrate, jitter/smoothing bounds, bounded queues, E2E latency. |
| `call_timeout.rs` | 1 | Unanswered offer → negotiation timeout. |
| `call_logging_policy.rs` | 1 | Media hot paths have no log statements. |
| `voice_acceptance.rs` | 1 | Voice acceptance full flow (two endpoints). |
| `no_recording.rs` | 1 | Call media source has no filesystem write path. `F` |
| `test_image_cache_persistence.rs` | 2 | Image cache rehydrates after restart; parallel-safe directory creation. `R` |
| `test_image_send_download.rs` | 1 | Image send → download round trip. `D` |
| `test_image_receiver_download.rs` | 1 | Receiver downloads image entry. `D` |
| `test_multi_image_burst.rs` | 1 | Three-remote-image burst. |
| `test_image_pipeline_guard.rs` | 2 | Image sharing stays on its pipeline; generic file send owns offer path. |
| `test_user_uploaded_gif.rs` | 6 | GIF/PNG/MP4 attachment round trips, wire has no provider fields, GIF size cap, gif extension. `F` |
| `compression_integration.rs` | 29 | Image compression matrix (formats, downscale, quality, no-op paths). |
| `image_optimizer_integration.rs` | 18 | Optimizer input matrix (exif orientation, animated-gif first frame, corrupt bytes). `F` |
| `svg_render_proof.rs` | 3 | Vendored twemoji SVG exists/renders at sizes. |

**Image transfer overlap note:** `test_image_iced_gui_flow`, `test_image_receiver_download`,
`test_image_send_download`, `test_multi_image_burst` each run a full multi-instance
GUI transfer and cover the *same* send→receive→thumbnail invariant with different
entry points (GUI flow vs direct from receiver vs receiver-download vs N-image burst).
These are the heaviest runtime cost in the suite and are near-identical in setup.
**Candidate follow-up (do not block this task):** keep `test_image_send_download` (or
the GUI flow) as the canonical scenario and express the other entry points as
table-driven *variants* of one transfer helper. Do **not** remove the `D` (receiver-
vs-sender-initiated) and burst coverage. Because these hang on DEBSRV
(`RelayMode::Default` + `online()` — see `debsrv-integration-test-gate.md`), any
consolidation must be landed **and verified against the deterministic/prewarm
harness**, not just against these binaries.

## 9. Shared fixtures / harnesses (cross-cutting, not a capability)

| File | Tests | Purpose |
|------|-------|---------|
| `tests/support/` | — | **Shared integration-test support from BORU-TEST-010**: `peers`, `net`, `timeout`, `storage`, `wait`, `fault`. Use these instead of per-file scaffolding. |
| `test_deterministic_harness.rs` | 30 | `DeterministicHarness` (FaultConfig / EventPlan / ReproGuard) — the canonical deterministic + fault-injection harness. Rich fault injection stays here, not duplicated. `F` |
| `test_deterministic_discovery_integration.rs` | 20 | Deterministic discovery publish policy (minute gate, backoff, dedup, shutdown). |
| `test_fixture.rs` | 16 | `TwoPeerFixture` (deterministic identities, in-memory discovery) + its baseline tests. |
| `test_catalogue_harness.rs` | 24 | `CatalogueHarness` + coverage (see room/catalogue). |
| `test_branding_rename.rs` | 28 | Boru-core rename integrity (crate names, ALPN re-exports unchanged). |
| `protocol_registration.rs` | 2 | Every existing protocol registered at GUI startup; router ALPN routing. |
| `gen_stress_data.rs` / `generate_test_images.py` | — | Data generators for stress/image tests. |
| `test_debug.rs` / `test_simple.rs` | 2 | Minimal smoke probes. |

---

## Required smoke/regression matrix

The **minimum required (smoke)** gate before any post-refactor merge, in priority
order. This is the "keep broad coverage while reducing cost" target:

| # | Command (via `rb` on DEBSRV) | What it guards | Why canonical |
|---|------------------------------|----------------|---------------|
| 1 | `rb check --bin boru --features gui,video-playback,terminal` | Compiles with the shipping feature set | Fastest whole-tree signal |
| 2 | `rb test --test test_deterministic_harness` | Deterministic + fault-injection baseline | Canonical deterministic harness |
| 3 | `rb test --test test_two_peers_exchange` | Two-peer exchange (shared support) | Smallest real end-to-end mesh |
| 4 | `rb test --test test_two_peers_relay` | Relay-path two-peer flow | Relay connectivity |
| 5 | `rb test --test test_three_peer_mesh` | 3-peer mesh + gossip router | Multi-peer mesh formation |
| 6 | `rb test --test test_download_initiation_integration` | Download initiation preconditions | Consolidated table-driven case; fast, no net |
| 7 | `rb test --test test_message_lifecycle` | Delivery state machine incl. restart | Canonical FSM coverage |

**Full regression matrix** = every file in the capability tables above. The smoke set
above is the *bounded* subset chosen to catch structural regressions before paying for
the full (multi-instance, GUI) suite. The full suite must be run on a feature-merge
gate, one `--test` binary per invocation, `timeout 240` per binary (see
`debsrv-integration-test-gate.md` — ~12 suites hang on `RelayMode::Default` +
`online()` and must use the prewarm/deterministic harness instead).

## Consolidation log

**Done in this task (BORU-TEST-011):**
- `tests/test_download_initiation_integration.rs`: 8 redundant error cases folded
  into **2 table-driven tests**:
  - `error_when_conflicting_download_state_exists_is_rejected_with_that_state`
    (replaces 5 `DownloadAlreadyExists` tests: complete / failed / queued /
    downloading / verifying). Preserves *all* original assertions and strengthens
    them: every case now also asserts the reported blocking id equals the real
    row id and that the rejected initiation leaves the original row intact.
  - `error_when_file_metadata_invalid_rejects_with_reason` (replaces 3
    `FileMetadataInvalid` tests: empty display name / empty MIME type / zero size).
    Preserves the per-field corruption and reason-substring assertions.
  - Test count in this file: 21 → 15. No unique coverage removed; all rows still
    run a fresh in-memory storage + catalogue seed.

**Candidate follow-ups (recorded, not done — each needs its own task/PR):**
1. **Catalogue permission-rule matrix** (files §4): pull the friend/non-friend/
   blocked/grant/deny visibility-rule table into one canonical harness and
   re-derive the other harnesses from it, preserving each file's `R`/`D` coverage.
2. **Image transfer entry points** (§8): keep one canonical
   send→receive→thumbnail scenario as a table-driven transfer helper and express
   the GUI / receiver-download / burst entry points as variants; verify via the
   deterministic/prewarm harness (the current binaries hang on DEBSRV).
3. **Extended-length-file download coverage** shows up in both
   `test_download_integration` and `test_normal_downloads`; fold the size matrix
   into one table-driven download-size case.

**Rules for any future consolidation** (from PDF BORU-TEST-011 §Agent rules):
- Do not remove `F` (fault), `D` (directionality), or `R` (restart) coverage.
- Consolidate only after *equivalent or better* coverage exists; one concern per PR.
- Reuse `tests/support/` and the deterministic harness rather than re-scaffolding.
- Land and verify against the deterministic/prewarm harness, not just a suite that
  hangs on public relay.
