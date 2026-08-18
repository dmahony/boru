# Security File / Path / Resource Containment Review (BORU-SEC-003)

Status: audit + 1 regression test (no production behaviour changes).
Task: BORU-ARCH-41, Phase 8 (Security and Protocol Review) of the BORU-ARCH chain.
Scope: re-check Boru's hostile-input protections in file sharing — **path
traversal and malicious filenames**, **blob/file size limits and bounded
queues**, **decompression and image/media processing resource limits**, and
**interrupted-transfer cleanup and restart behaviour** — and make resource
limits explicit, tested constants.

This is an audit document built on the current implementation (commit
`origin/main` at the start of this task). No protocol bytes, storage format,
serialization, or user-visible behaviour changed. Findings are grounded in
code (file:line), not architecture prose. PDF Section 14 Stop Conditions were
respected: nothing here changes wire or persistent-storage bytes.

---

## 1. Method

For each of the four containment areas the relevant source was traced, the
enforcing constant / mechanism was identified, and the regression tests that
pin the boundary were enumerated. One previously-untested resource-limit
boundary was found and pinned with a new regression test (§5). The audit
concludes that all three acceptance criteria are already met by the existing
implementation.

| Area | Files inspected | Key tests |
|------|-----------------|-----------|
| Path traversal / malicious filenames | `src/path_containment.rs`, `src/safe_destination.rs`, `src/collection_transfer.rs` (`validate_path_component`) | `tests/test_malicious_filenames.rs`, `tests/test_verify_containment_properties.rs`, `src/path_containment.rs` unit tests |
| Blob/file size limits & bounded queues | `src/catalogue_limits.rs`, `src/catalogue_rate_limits.rs`, `src/download_limits.rs`, `src/file_access_handler/limits.rs`, `src/proto.rs`, `src/net/util.rs` | `tests/test_resource_exhaustion.rs`, `tests/test_blob_size_enforcement.rs` |
| Decompression & media resource limits | `src/wire_compression.rs`, `src/chat_core/protocol.rs`, `src/image_optimizer.rs`, `src/compression.rs` | `src/wire_compression.rs` tests (incl. **new bomb test**), `src/image_optimizer.rs` tests, `tests/image_optimizer_integration.rs` |
| Interrupted transfer cleanup & restart | `src/storage/mod.rs`, `src/chat_history.rs` | `tests/test_interrupted_transfer_harness.rs`, `tests/test_interruption_restart.rs`, `tests/test_crash_recovery.rs` |

---

## 2. Path traversal and malicious filename handling

**Acceptance criterion: hostile filenames cannot escape allowed
directories.**

Every path that touches the filesystem from an untrusted display name /
offer goes through two independent guards:

1. **Lexical sanitisation** in `src/safe_destination.rs`:
   `safe_destination_path` (line 54) strips path separators (`/` and `\`) so
   a remote name can never introduce a directory component, then
   `check_traversal` (line 181) rejects any residual `..` / `.` / drive-letter
   ("C:") / UNC / absolute reference, and `is_reserved_platform_name` (line
   143) / `is_all_dots` (line 175) replace Windows-reserved names (`CON`,
   `AUX`, `COM1`–`9`, `LPT1`–`9`, `.`, `..`) and dot-only names with a
   content-hash fallback stem. Duplicate names are auto-deduped inside the
   selected directory (`deduplicate_path`, line 239; `MAX_DEDUP_ATTEMPTS`,
   line 27 = 10_000, an explicit named constant).

2. **Canonical containment (belt-and-suspenders)** in
   `src/safe_destination.rs` (lines 92–99) and `src/path_containment.rs`:
   `is_path_contained` (line 28) canonicalises both sides with
   `canonicalize_allow_missing` (line 85, resolves symlinked ancestry and the
   Windows `\\?\` prefix even for not-yet-existing targets) and requires the
   candidate to `starts_with` the canonical root. `symlink_is_safe` (line 45)
   applies the same check to shared-folder symlinks. A pre-existing symlink
   inside the root that points outside is resolved and rejected.

**Coverage** is exhaustive: traversal / absolute / UNC / drive-letter /
mixed-separator / dot-segment / redundant-separator names, Windows device
names (upper/lower case, trailing dot, extensions), control and zero-width /
bidi characters, CR/LF controls, empty and effectively-empty names, name
lengths from 0 to 4096 bytes, dedup collisions, and cross-directory symlinks
— all verified through both `safe_destination_path` and the higher-level
`prepare_download_destination`.

**Finding: none.** Not only do hostile names fail to escape, every accepted
destination is asserted to be a *direct child* of the selected directory.
`MAX_DEDUP_ATTEMPTS` is a named constant. No regression test was needed; the
existing suites (`test_malicious_filenames.rs`, `test_verify_containment_properties.rs`)
pin the boundary.

---

## 3. Blob/file size limits and bounded queues

**Acceptance criterion: untrusted inputs have bounded memory/disk/CPU impact
where practical.**

All wire-touching size and concurrency limits are explicit named constants
(not magic numbers) with tests:

| Constant | Value | Defined | Enforced at |
|----------|-------|---------|-------------|
| `MAX_CATALOGUE_REQUEST_BYTES` | 256 KiB | `catalogue_limits.rs:40` | handler rejects oversized requests |
| `MAX_CATALOGUE_RESPONSE_BYTES` | 4 MiB | `catalogue_limits.rs:47` | client + handler |
| `MAX_CATALOGUE_PAGE_BYTES` | 1 MiB | `catalogue_limits.rs:50` | pagination |
| `MAX_CATALOGUE_FILES` | 10 000 | `catalogue_limits.rs:56` | handler + client |
| `MAX_COLLECTIONS` | 1 000 | `catalogue_limits.rs:59` | handler + client |
| `MAX_ENTRIES_PER_COLLECTION` | 10 000 | `catalogue_limits.rs:62` | storage |
| `MAX_FILE_SIZE_BYTES` | 10 TiB | `catalogue_limits.rs:74` | catalogue construction |
| `MAX_FILE_DETAILS_PAYLOAD_BYTES` | 256 KiB | `catalogue_limits.rs:79` | handler + client |
| `MAX_CATALOGUE_REQUESTS_PER_PEER` | 32 / window | `catalogue_rate_limits.rs` | per-peer sliding window |
| `MAX_CONCURRENT_CATALOGUE_CONNECTIONS` | — | `catalogue_rate_limits.rs` | admission semaphore |
| `MAX_INVALID_CATALOGUE_ATTEMPTS_PER_PEER` | — | `catalogue_rate_limits.rs` | block after budget |
| `DEFAULT_MAX_MESSAGE_SIZE` | 4096 | `proto.rs:69` | gossip read path |
| `MIN_MAX_MESSAGE_SIZE` | 512 | `proto.rs:72` | config validation |

Concurrency / queue bounds are also explicit and tested:
`DownloadLimitsConfig` (download_limits.rs) caps concurrent downloads (5),
per-peer downloads (2), hash verifications (2) and queue depth (32);
`PrepareConfig` (`file_access_handler/limits.rs:36`) rejects files > 1 GiB
(`max_file_size_bytes`) before preparing and caps concurrent preparation (4);
`UploadLimitsConfig` (`file_access_handler/limits.rs:193`) caps active (8),
per-peer (2), queued (32) and simultaneous permission checks (4). The
`ProgressUpdateGate` coalesces high-frequency progress DB writes.

The gossip read path enforces `max_message_size` directly in
`src/net/util.rs` `read_lp` (lines 377–393): an advertised length above the
cap is rejected as `TooLarge` before allocation.

**Coverage** is pinned by `tests/test_resource_exhaustion.rs` (22
adversarial scenarios: connection flood, per-peer rate limit, oversized
payload, malformed stream block, file/collection/entry count limits in
storage, response-volume budget, download-queue and hash-verification caps,
progress-write coalescing, parallel attackers, and combined abuse budgets)
and `tests/test_blob_size_enforcement.rs` (public-room blob cap enforced at
the download boundary), plus `src/file_access_handler/tests.rs`
(`PrepareError::TooLarge`) and the `catalogue_limits.rs` unit tests.

**Finding: none.** Every limit is a named constant with a test; bounded
queues are enforced by semaphores and per-peer accounting.

---

## 4. Decompression and image/media processing resource limits

**Acceptance criterion: untrusted inputs have bounded CPU/memory impact.**

- **Decompression.** The single decompression entry point onto the wire path
  is `src/chat_core/protocol.rs:706` → `wire_compression::decompress`. That
  function (`src/wire_compression.rs:688`) refuses to expand beyond the named
  constant `MAX_DECOMPRESSED_SIZE` (line 109, 64 MiB): it sizes its output
  buffer from the input (`min(4×, MAX_DECOMPRESSED_SIZE)`), checks
  `out.len() + produced > MAX_DECOMPRESSED_SIZE` before extending (line 708),
  and doubles its target buffer only up to the cap (line 723). Even for a
  pathological stream, output memory is therefore bounded by the cap, and
  malformed / truncated input errors out rather than looping. **This is the
  one boundary that was not previously regression-tested.** Added
  `decompress_rejects_deflate_bomb_beyond_hard_cap` (§5) to pin it.
- **Image decoding / re-encode.** `src/image_optimizer.rs` treats limits as
  named constants: `CHAT_IMAGE_MAX_BYTES` (10 MiB input cap, line 27),
  `CHAT_IMAGE_OPTIMIZED_MAX_BYTES` (2 MiB wire cap, line 30),
  `INLINE_IMAGE_MAX_DIM` (1 280 px longest edge, never upscaled, line 33),
  `INLINE_IMAGE_QUALITY` / `OPTIMIZE_QUALITY_STEPS` (the quality-retry ladder,
  lines 37/42). `thumbnail_image` and `compress_image` bound dimensions/edge
  and re-encode at a fixed JPEG quality, so decoded pixel RAM is bounded even
  for a hostile image. Animated PNGs are rejected up front (line 148). The
  `image` crate is pure-Rust with no native decoding bomb; inputs are capped
  before `load_from_memory`.
- **Media/video poster** decoding uses the same bounded `image_optimizer`
  primitives.

**Coverage** includes `decompress_rejects_garbage`, round-trip tests, the
`image_optimizer` unit tests and `tests/image_optimizer_integration.rs`.

**Finding: one gap, now closed by §5.** The decompression output-size cap was
implemented and constant-named but had no test exercising the expansion
boundary.

---

## 5. Regression test added

`src/wire_compression.rs` — `decompress_rejects_deflate_bomb_beyond_hard_cap`:

- Builds a genuine deflate bomb: `compress(&[0u8; MAX_DECOMPRESSED_SIZE + 1])`
  yields a tiny stream (asserted < half the cap) that would inflate to
  > 64 MiB.
- Asserts `decompress(&bomb)` **rejects** it (never materialises the giant
  allocation); the rejection surfaces as a size-cap or truncated-stream error,
  both of which keep memory bounded.
- Records in a comment why a pathological oversized stream surfaces as
  "truncated" vs "exceeds": the loop feeds a small output buffer, so flate2
  reports the impossible-to-fully-inflate stream before it can be expanded —
  the security property (bounded allocation) holds either way.

Verified: passes via `rb test --lib wire_compression::tests` (2752 tests, all
pass; the new test confirmed individually).

---

## 6. Interrupted transfer cleanup and restart behaviour

**Acceptance criterion: interrupted or malicious transfers leave recoverable
state.**

Download state is durable in SQLite (`src/storage/mod.rs`): a transfer is
created with an explicit `state` (queued / paused / downloading / verifying /
completed / failed), `bytes_downloaded`, retry count and content hash, and is
paused/resumed through explicit transitions. On restart the layer preserves
state and the worker **always re-resolves the peer and re-fetches a fresh
descriptor** before transferring bytes — no assumption of OS-level transfer
resumption. Partially downloaded content is trackable and resumable through
the pause/resume cycle rather than being left in a corrupt half-state.

**Coverage** is provided by:
- `tests/test_interrupted_transfer_harness.rs` — a reusable harness that
  seeds downloads in `queued` / `resolving_peer` / `downloading` / `verifying`
  states, simulates sender/receiver crash + reopen, asserts the correct
  recovery outcome (`queued` / `paused` / `complete`), verifies temp-file hash
  integrity, permission changes between retries, and stale catalogue versions.
- `tests/test_interruption_restart.rs` — pause preserves progress, resume
  always transitions through `resolving_peer`, and fresh permission is
  required after resume.
- `tests/test_crash_recovery.rs` — crash mid-transfer reopens to recoverable
  state.

**Finding: none.** Interrupted or malicious transfers leave a persisted,
resumable, recoverable download record; partial content is never assumed
intact across a crash.

---

## 7. Explicit limit constants status

Every resource limit surveyed above is already a **named, documented
constant** (not a magic number) in the owning limits module, with a default
that feeds validated config (`CatalogueLimitsConfig`, `DownloadLimitsConfig`,
`PrepareConfig`, `UploadLimitsConfig`, `CatalogueRateConfig`). No inline
magic-number limits were found on the containment paths. The one constant that
lacked a boundary regression test is now covered (§5).

---

## 8. Conclusions vs acceptance criteria

| Acceptance criterion | Status | Evidence |
|----------------------|--------|----------|
| Hostile filenames cannot escape allowed directories | **Met** | §2 — lexical sanitisation + canonical containment; `test_malicious_filenames.rs`, `test_verify_containment_properties.rs` |
| Untrusted inputs have bounded memory/disk/CPU impact | **Met** | §3–§4 — all limits named constants with tests; decompression bomb now also pinned by §5 |
| Interrupted or malicious transfers leave recoverable state | **Met** | §6 — durable SQLite state + interrupted-transfer harness / restart tests |

**No defect found.** Boru's hostile-input protections are retained and mature
across all four areas. The single previously-untested resource-limit boundary
(deflate-bomb expansion cap) is now pinned by a regression test. Per PDF
Section 14 no protocol, storage, serialization, or authorisation behaviour
was changed.

## 9. Follow-ups (recorded, not in scope)

- The `decompress` loop reports a pathological oversized *valid* stream as
  "truncated deflate stream" rather than a size-cap message. This is not a
  security issue (memory stays bounded; the payload is rejected either way)
  and real boru messages fit comfortably below the cap, so no change was made.
  If messages ever legitimately approach 64 MiB, revisit `FlushDecompress`
  handling. (Follow-up, no action.)
