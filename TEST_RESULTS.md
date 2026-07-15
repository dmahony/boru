# Test Results — Multi-Image Chat Regression & GUI Tests

**Task**: t_0ef147f5
**Date**: 2026-07-13
**Checkout**: `/home/dan/.hermes/kanban/boards/iroh-gossip-chat/workspaces/t_3d4e68eb` (branch `t_83367b85`, HEAD `7d0285d`)
**Command prefix**: `cargo test --features gui` (plus `net,test-utils` where needed)

---

## Summary

| Metric | Count |
|--------|-------|
| Test suites run | 22 |
| Total tests | ~563 |
| **Pass** | **~557** |
| **Fail** | **5** (1 NEW regression + 1 pre-existing + 3 pre-existing flakes) |
| **Timeout** | **1** (pre-existing flake) |
| **Build fail** | **1** (pre-existing, untracked modules) |

---

## 1. Lib Tests

### `cargo test --lib` (gui feature)
**386 PASS, 1 FAIL**

| Test | Status | Classification |
|------|--------|---------------|
| `chat_core::friend_ping::tests::test_add_and_remove_friend` | FAIL | PRE-EXISTING FLAKE — timing-dependent `Offline` vs `Unknown` assertion (line 490) |

### `cargo test --features "gui,net,test-utils" --lib -- --include-ignored`
**428 PASS, 1 FAIL** — same single flake failure, same cause.

---

## 2. Image/GUI Integration Tests

### `--test compression_integration`
**29 PASS, 0 FAIL** ✓ — All compression tests clean.

### `--test image_optimizer_integration`
**17 PASS, 1 FAIL**

| Test | Status | Classification |
|------|--------|---------------|
| `test_screenshot` | **FAIL** | **NEW REGRESSION** |
| All other 17 tests | PASS | ✓ |

**Failure detail**: `tests/image_optimizer_integration.rs:92` — asserts `width == 1920` but working tree changed `INLINE_IMAGE_MAX_DIM` from 1920 to 1280. The test expects the 1920px screenshot fixture to remain at 1920px after compression, but the new max-dim cap downsamples it to 1280px.

### `--test test_multi_image_burst`
**1 PASS, 0 FAIL** ✓ — `test_three_remote_image_burst` passes. All images queue and download successfully.

### `--test test_image_iced_gui_flow`
**1 PASS, 0 FAIL** ✓ — `test_iced_gui_image_flow_exact` passes.

### `--test test_image_send_download`
**1 PASS, 0 FAIL** ✓ — `test_image_send_and_download` passes.

### `--test test_image_receiver_download`
**1 PASS, 0 FAIL** ✓ — `test_receiver_downloads_image_entry` passes.

### `--test test_image_cache_persistence`
**1 PASS, 1 FAIL**

| Test | Status | Classification |
|------|--------|---------------|
| `concurrent_directory_creation_is_safe_for_parallel_saves` | PASS | ✓ |
| `image_cache_round_trip_rehydrates_after_restart_and_blocks_other_users` | **FAIL** | **PRE-EXISTING** |

**Failure detail**: `tests/test_image_cache_persistence.rs:71` — asserts `Some(bytes)` but `#[serde(skip)]` on `HistoryEntry.image_bytes` means deserialized entries return `None`. Test was never updated for the serde(skip) change.

### `--test test_performance_regression`
**8 PASS, 0 FAIL** ✓ — All performance benchmarks pass including `test_imageshare_processing_no_degradation`.

### `--test test_iced_chat_flow`
**1 PASS, 0 FAIL** ✓ — `test_iced_chat_exact_flow` passes.

### `--test test_full_chat_list_flow`
**1 PASS, 0 FAIL** ✓ — `test_full_chat_list_flow` passes.

### `--test verify_gui_bootstrap`
**TIMEOUT** (180s) — **PRE-EXISTING FLAKE** — Simulated peers never form neighbors in test environment.

---

## 3. Other Integration Tests

### All PASS
| Test binary | Tests | Result |
|-------------|-------|--------|
| `test_friend_request_e2e` | 18 | 18 PASS ✓ |
| `test_friend_ticket_persistence` | 1 | 1 PASS ✓ |
| `test_signed_gossip_flow` | 1 | 1 PASS ✓ |
| `test_online_user_list` | 3 | 3 PASS ✓ |
| `test_stale_bootstrap` | 5 | 5 PASS ✓ |
| `mailbox` | 10 | 10 PASS ✓ |
| `sim` (big_hyparview, big_burst, etc.) | 4 | 4 PASS ✓ |
| `test_message_lifecycle` | 39 | 39 PASS ✓ |
| `test_message_transfer` | 1 | 1 PASS ✓ |
| `three_peer_mesh` | 3 | 3 PASS ✓ |

### Partial FAIL
| Test binary | Tests | Result |
|-------------|-------|--------|
| `test_local_address_lookup` | 5 | 4 PASS, 1 FAIL (PRE-EXISTING FLAKE: `test_mdns_creation_and_subscribe` — timing-dependent mDNS key ordering) |
| `test_no_bootstrap` | 1 | 0 PASS, 1 FAIL (PRE-EXISTING FLAKE: `test_no_bootstrap_peer_still_receives` — peers never join after 60 ticks) |

### BUILD FAIL
| Test binary | Status | Classification |
|-------------|--------|---------------|
| `test_public_lobby_integration` | **BUILD FAIL** | **PRE-EXISTING** — references untracked modules not declared in `src/lib.rs` (`discovery_record`, `public_room_tracker`, missing `async_trait`/`distributed_topic_tracker` deps) |

---

## 4. Regression Classification

### ✅ 557 tests PASS (no regression)
- All image compression, resizing, and optimization tests pass (29/29 compression, 17/18 image_optimizer)
- Multi-image burst queue + download works correctly
- All GUI chat flow tests pass (iced_chat flow, image send/receive/download, full chat list)
- All performance regression benchmarks pass (no degradation from multi-image changes)
- All core lib tests pass (except pre-existing flake)
- All networking, mailbox, friend request, message lifecycle tests pass

### 🔴 1 NEW REGRESSION
| Issue | File | Line | Detail |
|-------|------|------|--------|
| `test_screenshot` asserts 1920px max | `tests/image_optimizer_integration.rs` | 85-93 | Working tree changed `INLINE_IMAGE_MAX_DIM` to 1280px; test expects 1920px fixture to remain at 1920px |

**Fix needed**: Update the assertion in `test_screenshot` to expect 1280px width (matching the new max dimension), or retain the 1920px fixture dimension if the intent is for large screenshots to stay at their original size.

### 🟡 1 PRE-EXISTING FAILURE (non-flake)
| Issue | File | Line | Detail |
|-------|------|------|--------|
| `image_cache_round_trip` asserts `Some(bytes)` | `tests/test_image_cache_persistence.rs` | 71 | serde(skip) on HistoryEntry.image_bytes means deserialized entries return `None`. Test never updated. |

### 🟠 4 PRE-EXISTING FLAKES
| Issue | File | Line | Detail |
|-------|------|------|--------|
| `test_add_and_remove_friend` | `src/chat_core/friend_ping.rs` | 490 | Timing-dependent `Offline` vs `Unknown` |
| `test_no_bootstrap_peer_still_receives` | `tests/test_no_bootstrap.rs` | 112 | Peers never join after 60 ticks |
| `test_mdns_creation_and_subscribe` | `tests/test_local_address_lookup.rs` | 148 | mDNS pubkey comparison timing |
| `verify_gui_bootstrap` | `tests/verify_gui_bootstrap.rs` | — | Network timeout (180s) |

### ⚠️ 1 BUILD FAIL
| Issue | Detail |
|-------|--------|
| `test_public_lobby_integration` | References untracked modules not declared in lib.rs; missing crate deps |

---

## 5. Test Commands Used

```bash
# Lib tests
cargo test --lib
cargo test --features "gui,net,test-utils" --lib -- --include-ignored

# Image/GUI integration tests
cargo test --features gui --test compression_integration
cargo test --features gui --test image_optimizer_integration
cargo test --features gui --test test_multi_image_burst
cargo test --features gui --test test_image_iced_gui_flow
cargo test --features gui --test test_image_send_download
cargo test --features gui --test test_image_receiver_download
cargo test --features gui --test test_image_cache_persistence
cargo test --features gui --test test_performance_regression
cargo test --features gui --test test_iced_chat_flow
cargo test --features gui --test test_full_chat_list_flow
cargo test --features gui --test verify_gui_bootstrap

# Network-dependent tests (need net+test-utils features)
cargo test --features "gui,net,test-utils" --test three_peer_mesh
cargo test --features "gui,net,test-utils" --test test_message_lifecycle
cargo test --features "gui,net,test-utils" --test test_message_transfer
cargo test --features "gui,net,test-utils" --test test_signed_gossip_flow
cargo test --features "gui,net,test-utils" --test test_no_bootstrap
cargo test --features "gui,net,test-utils" --test test_local_address_lookup
cargo test --features "gui,net,test-utils" --test test_stale_bootstrap
cargo test --features "gui,net,test-utils" --test test_online_user_list
cargo test --features "gui,net,test-utils" --test test_friend_request_e2e
cargo test --features "gui,net,test-utils" --test test_friend_ticket_persistence
cargo test --features "gui,net,test-utils" --test mailbox
cargo test --features "gui,net,test-utils" --test sim

# Public lobby (build fail)
cargo test --features gui --test test_public_lobby_integration
```
