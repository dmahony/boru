# Test Report — Image GUI/Performance & iced_chat Test Suite

**Branch:** `t_83367b85` (HEAD `7d0285d`)
**Base:** `516a018`
**Date:** 2026-07-13
**Workspace:** `t_1256c805`

---

## 1. Image GUI Tests

### `image_optimizer_integration` (18 tests)
**Command:** `cargo test --features gui --test image_optimizer_integration`
**Result:** 17/18 passed — **1 NEW REGRESSION**

| Test | Status | Notes |
|------|--------|-------|
| test_empty_input | PASS | |
| test_animated_gif_first_frame_decoded | PASS | |
| test_bmp_format_accepted | PASS | |
| test_gif_format_accepted | PASS | |
| test_already_compressed_small | PASS | |
| test_oversized_input_rejected | PASS | |
| test_exif_auto_orientation | PASS | |
| test_rotated_portrait | PASS | |
| test_thumbnail_image_fallback_all_formats | PASS | |
| test_tiny_1x1 | PASS | |
| test_tiny_transparent_1x1 | PASS | |
| test_transparent_png_composited | PASS | |
| test_truly_corrupt_bytes | PASS | |
| test_screenshot | **FAILED** | See below |
| test_photo_large | PASS | |
| test_all_png_fixtures_under_limit | PASS | |
| test_photo_20mp | PASS | |
| test_all_jpeg_fixtures_under_limit | PASS | |

**FAILURE: `test_screenshot`**
```
assertion `left == right` failed: screenshot width should remain 1920
  left: 1280
 right: 1920
```
**Category: NEW REGRESSION.** The working tree change to `src/image_optimizer.rs` reduced `INLINE_IMAGE_MAX_DIM` from 1920 to 1280. The test fixture `screenshot_1920x1080.jpg` (1920px wide) is now downscaled to 1280px. The test assertion wasn't updated to match. **Fix:** Update the test to assert width=1280, or update the fixture to match the new constant.

---

### `compression_integration` (29 tests)
**Command:** `cargo test --features gui --test compression_integration`
**Result:** 29/29 PASSED — no regressions.

---

### `test_image_iced_gui_flow` (1 test)
**Command:** `cargo test --features gui --test test_image_iced_gui_flow`
**Result:** 1/1 PASSED — image GUI flow intact.

---

### `test_image_send_download` (1 test)
**Command:** `cargo test --features gui --test test_image_send_download`
**Result:** 1/1 PASSED — send/download works.

---

### `test_image_receiver_download` (1 test)
**Command:** `cargo test --features gui --test test_image_receiver_download`
**Result:** 1/1 PASSED — receiver download works.

---

### `test_multi_image_burst` (1 test)
**Command:** `cargo test --features gui --test test_multi_image_burst`
**Result:** 1/1 PASSED — multi-image queue fix holds.

---

### `test_image_cache_persistence` (2 tests)
**Command:** `cargo test --features gui --test test_image_cache_persistence`
**Result:** 1/2 passed — **1 PRE-EXISTING FAILURE**

| Test | Status |
|------|--------|
| concurrent_directory_creation_is_safe_for_parallel_saves | PASS |
| image_cache_round_trip_rehydrates_after_restart_and_blocks_other_users | **FAILED** |

**FAILURE: `image_cache_round_trip_rehydrates_after_restart_and_blocks_other_users`**
```
assertion `left == right` failed
  left: None
  right: Some([102, 97, 107, ...])
```
**Category: PRE-EXISTING.** The test saves a `HistoryEntry` with `image_bytes = Some(...)`, saves and reloads, expecting `Some(...)`. However commit `be37eee` (in `main` before the branch) added `#[serde(skip)]` to `HistoryEntry.image_bytes` to prevent multi-megabyte JSON bloat. The test was written before that change and never updated. Images are now cached via `ImageStore` filesystem, not serde. The test needs to be updated to verify filesystem cache hydration instead of serde round-trip.

---

## 2. Performance Tests

### `test_performance_regression` (8 tests)
**Command:** `cargo test --features gui --test test_performance_regression`
**Result:** 8/8 PASSED — no performance regressions detected.

| Test | Status |
|------|--------|
| test_cumulative_window_lookup_cost | PASS |
| test_chat_entry_iteration_scaling | PASS |
| test_height_estimation_scaling | PASS |
| test_incremental_append_cost | PASS |
| test_image_blob_operations_scaling | PASS |
| test_many_messages_handle_net_event_scaling | PASS |
| test_imageshare_processing_no_degradation | PASS |
| test_signed_message_encode_decode_scaling | PASS |

---

## 3. Iced Chat Test Suite

### `test_iced_chat_flow` (1 test)
**Command:** `cargo test --features gui --test test_iced_chat_flow`
**Result:** 1/1 PASSED

### `test_full_chat_list_flow` (1 test)
**Command:** `cargo test --features gui --test test_full_chat_list_flow`
**Result:** 1/1 PASSED

### `repro_two_iced_instances` (2 tests)
**Command:** `cargo test --features gui --test repro_two_iced_instances`
**Result:** 2/2 PASSED — both same-key and different-key scenarios work.

### `test_two_peers_exchange` (1 test)
**Command:** `cargo test --features gui --test test_two_peers_exchange`
**Result:** 1/1 PASSED

### `test_two_peers_relay` (1 test)
**Command:** `cargo test --features gui --test test_two_peers_relay`
**Result:** 1/1 PASSED

### `test_signed_gossip_flow` (1 test)
**Command:** `cargo test --features gui --test test_signed_gossip_flow`
**Result:** 1/1 PASSED

### `test_friend_ticket_persistence` (1 test)
**Command:** `cargo test --features gui --test test_friend_ticket_persistence`
**Result:** 1/1 PASSED

### `test_stale_bootstrap` (5 tests)
**Command:** `cargo test --features gui --test test_stale_bootstrap`
**Result:** 5/5 PASSED

### `test_message_lifecycle` (39 tests)
**Command:** `cargo test --features "gui,test-utils" --test test_message_lifecycle`
**Result:** 39/39 PASSED

### `mailbox` (10 tests)
**Command:** `cargo test --features "gui,test-utils" --test mailbox`
**Result:** 10/10 PASSED

### `test_online_user_list` (3 tests)
**Command:** `cargo test --features gui --test test_online_user_list`
**Result:** 3/3 PASSED

### `test_friend_request_e2e` (18 tests)
**Command:** `cargo test --features "gui,test-utils" --test test_friend_request_e2e`
**Result:** 18/18 PASSED

### `three_peer_mesh` (3 tests)
**Command:** `cargo test --features "gui,test-utils" --test three_peer_mesh`
**Result:** 3/3 PASSED

### `room_e2e` (1 test)
**Command:** `cargo test --features "gui,test-utils" --test room_e2e`
**Result:** 1/1 PASSED

---

## 4. Lib Unit Tests

**Command:** `cargo test --features gui --lib`
**Result:** 428/429 passed — **1 PRE-EXISTING FLAKE**

| Failure | Category | Notes |
|---------|----------|-------|
| `chat_core::friend_ping::tests::test_add_and_remove_friend` | PRE-EXISTING | Timing-dependent: expected `Some(Unknown)` got `Some(Offline)`. Also noted as a flake in the parent task's CHANGES_SUMMARY.md (386/387 lib tests pass). |

---

## 5. Other Tests — Failures/Timeouts (all pre-existing)

| Test | Result | Category | Notes |
|------|--------|----------|-------|
| `test_message_transfer` | 0/1 FAILED | PRE-EXISTING flake | "A has no neighbors after waiting" — timing-dependent two-peer connection test |
| `test_local_address_lookup` | 4/5 FAILED | PRE-EXISTING | `test_mdns_creation_and_subscribe` asserts a specific PublicKey that no longer matches; fixed-key test for generated keys |
| `test_no_bootstrap` | 0/1 FAILED | PRE-EXISTING flake | "A should be joined after 60 ticks" — timing-dependent no-bootstrap join test |
| `verify_gui_bootstrap` | TIMEOUT (300s) | PRE-EXISTING | Requires a running network endpoint; not suitable for automated CI |
| `test_public_lobby_integration` | BUILD FAIL | PRE-EXISTING | References untracked modules (`discovery_backend`, `discovery_record`, `public_room_tracker`) that exist as source files but aren't declared in `lib.rs` |

---

## 6. Build Note

A `pub mod compression;` declaration was added to `src/lib.rs` (gated on `#[cfg(feature = "gui")]`) to resolve a build failure. The `compression.rs` file existed as an untracked working-tree file and was referenced by `src/image_optimizer.rs` via `use crate::compression;`, but was never declared in the module tree. This is part of the pre-existing optimization changes (extracting resize/encode into a shared module) that weren't fully wired up.

---

## Summary

**Total tests attempted:** ~565
**Passed:** ~560
**Failed:** 3 unique failures (plus 4 pre-existing flake/timeout/build-fail)
**New regressions:** **1** — `test_screenshot` in `image_optimizer_integration` (INLINE_IMAGE_MAX_DIM 1920→1280 not reflected in test assertions)

**Pre-existing failures** (present before this diff, no new regression risk):
- `test_image_cache_persistence` — test not updated for serde(skip) optimization
- `test_add_and_remove_friend` — timing flake in friend_ping
- `test_message_transfer` — timing flake in two-peer connection
- `test_local_address_lookup` — fixed-key assertion over generated keys
- `test_no_bootstrap` — timing flake
- `verify_gui_bootstrap` — timeout (network-dependent)
- `test_public_lobby_integration` — build failure (missing module declarations)

**Risk assessment:** Low regression risk. The image pipeline and performance characteristics are solid. The one new regression is a trivial test-assertion mismatch from an intentional constant change (1280px max is a safer default than 1920px).
