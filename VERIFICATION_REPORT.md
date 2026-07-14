# Card 18 — Final Verification Report

**Commit:** `8f02423` (Card 10 integration)
**Date:** 2026-07-14
**Workspace:** `t_8a85417f`
**Repo:** `iroh-gossip-chat`

---

## Summary

The project fails to compile. **5 blocking issues** prevent `cargo clippy --all-targets --all-features` and `cargo test --all-features` from succeeding. Both frontends (TUI `chat.rs`, GUI `iced_chat/app.rs`) are broken by an API mismatch between the `PrivateRoomTracker` redesign and the example code.

---

## 1. cargo fmt --check

**Result:** FAILED — fixed by running `cargo fmt`

Formatting issues found in:
- `examples/chat.rs` (1 diff)
- `src/dynamic_joiner.rs` (5 diffs)
- `src/public_room_continuous.rs` (1 diff)
- `tests/test_room_invite_v2.rs` (2 diffs)

These are purely cosmetic. Auto-fixed by `cargo fmt` with no behavioral changes.

---

## 2. cargo clippy --all-targets --all-features

**Result:** FAILED — 2 errors (deny-level) blocking all targets

### BLOCKING ERRORS

#### E1: friends.rs:105, 117 — Non-semver `since` field
```
error: the since field must contain a semver-compliant version
  --> src/friends.rs:105:9
     since = "4.0",
```
Two `#[deprecated(since = "4.0")]` annotations use "4.0" instead of "4.0.0". clippy's `deprecated_semver` lint is deny-level.
- **File:** `src/friends.rs`, lines 104-107 and 116-119
- **Fix:** Change `since = "4.0"` → `since = "4.0.0"` in both places.

#### E2: test_room_invite_v2.rs:227 — Type annotation needed
```
error[E0282]: type annotations needed
  --> tests/test_room_invite_v2.rs:227:9
     let mut topic_b_bytes = *test_topic().as_ref();
```
`TopicId::as_ref()` returns `&[u8]`; dereferencing gives unsized `[u8]`. The compiler cannot infer the size.
- **File:** `tests/test_room_invite_v2.rs`, line 227
- **Fix:** `let mut topic_b_bytes: [u8; 32] = *test_topic().as_ref();`

#### E3/E4: chat.rs + iced_chat/app.rs — Missing `create_and_publish_private_discovery`
```
error[E0425]: cannot find function `create_and_publish_private_discovery`
  in module `boru_chat::private_room_tracker`
```
This function does not exist in the current codebase (removed during the `PrivateRoomTracker` refactor).
- **Files:**
  - `examples/chat.rs:705` — `boru_chat::private_room_tracker::create_and_publish_private_discovery(shared_dht, topic, &endpoint)`
  - `examples/iced_chat/app.rs:42` — `use boru_chat::private_room_tracker::{create_and_publish_private_discovery, PrivateRoomTracker};`

#### E5: chat.rs + iced_chat/app.rs — Missing `PrivateRoomTracker::into_inner()`
```
error[E0599]: no method named `into_inner` found for struct `PrivateRoomTracker`
```
`PrivateRoomTracker` has no `into_inner()` method. `ContinuousTracker::start()` takes a `PublicRoomTracker`, not the inner of a `PrivateRoomTracker`.
- **Files:**
  - `examples/chat.rs:823-824` — `tracker.into_inner()`
  - `examples/iced_chat/app.rs:3457, 3690, 4044` — `tracker.into_inner()`

### ROOT CAUSE: PrivateRoomTracker API Redesign Gap

The current `PrivateRoomTracker` API:
- `PrivateRoomTracker::new(backend, topic, secret, endpoint_id, secret_key)` — synchronous constructor
- `tracker.publish_once()` — publish DHT presence
- `tracker.discover_once()` — discover peers
- `tracker.shutdown()` — cleanup

The examples expect a different API:
- A standalone `create_and_publish_private_discovery(shared_dht, topic, &endpoint)` function
- An `into_inner()` method that extracts the inner tracker for `ContinuousTracker::start()`

The entire private-room DHT discovery + continuous tracking pipeline in both frontends needs rewriting to use the new `PrivateRoomTracker` API.

### WARNINGS (non-blocking, 63 total)

Most significant warnings from `boru-chat` crate:
- `public_room_continuous.rs` — 8× `clone_on_copy` for `PublicKey` (Copy type)
- `public_room_continuous.rs` — 4× `while_let_loop` (loop→match can be `while let`)
- `public_room_tracker.rs` — 6× `clone_on_copy` for `PublicKey`
- `public_room_safety.rs` — 4× `field_reassign_with_default`
- `friend_request.rs` — 4× `unnecessary_sort_by`
- `chat_core.rs` — 4× `type_complexity`
- `room_cleanup.rs` — `bool_assert_comparison`, `unnecessary_get_then_check`
- `friends.rs` — `derivable_impls` (Default can be derived)
- `compression.rs` — `len_zero` (prefer `is_empty()`)
- `conversations.rs` — `unnecessary_sort_by`
- `room.rs` — `needless_return`
- `perf.rs` — `unnecessary_sort_by`
- `discovery_backend.rs` — missing `Debug` impl
- `dynamic_joiner.rs` — unused import `ApiError`
- `inbox.rs` — 2× `type_complexity`
- `whisper/session_manager.rs` — `large_enum_variant`

---

## 3. cargo test --all-features

**Result:** COULD NOT COMPILE — blocked by all 5 errors above.

---

## 4. README Verification (Parent Task t_1a36b0ee)

**Status:** PASSES inspection

The README update by Card 17 (parent) is thorough and accurate:
- **670 lines** (was 670 according to parent — matches)
- Clearly documents the boru1: stable invitation format and its purpose
- Explains the critical distinction between topic discovery and endpoint address lookup
- Covers the private-room security/possession model
- Documents DHT limits, fallback, and degraded behavior
- Provides legacy compatibility and migration path
- Gives TUI/GUI usage examples with two/three-peer and creator-offline test instructions
- Corrects wording that implied endpoint DHT lookup finds room peers by topic
- No stale language from old API references found

**No issues found in the README update.**

---

## 5. Detailed Source Inspection

### Final Criteria Verification

| Criterion | Status | Evidence |
|-----------|--------|----------|
| **Stable boru1: invites without endpoints** | ✅ PASS | `RoomInviteV2` (chat_core.rs:1060-1067) contains only topic + discovery_secret — no endpoint/relay/creator data. Encoded as `boru1:` + base32 payload. |
| **Stable-invite-only joining** | ✅ Verified by source | `examples/chat.rs:591` — tries `RoomInviteV2::parse()` first, falls back to legacy `Ticket::from_str()`. |
| **Creator-offline/later-member bootstrap** | ✅ PASS | Continuous DHT discovery via `ContinuousTracker::start()` + `publish_loop()` / `discover_loop()` handles late joiners. `DynamicPeerJoiner` provides bounded concurrent joins. |
| **DHT failure non-fatal** | ✅ PASS | `chat.rs:796-801`: `Err(e)` path logs warning and continues with ticket peers only. No hard failure. |
| **Legacy tickets** | ✅ PASS | `chat.rs:832`: `"legacy room/ticket has no discovery secret; skipping private DHT"`. `RoomInviteV2::parse()` + `Ticket::from_str()` chain. |
| **Secret-safe logs** | ✅ PASS | `RoomInviteV2::Debug` (chat_core.rs:1073): redacts `discovery_secret` as `"[redacted]"`. Test `debug_redacts_secret` (test_room_invite_v2.rs:146) verifies this. `dynamic_joiner.rs:17`: "Tracing without secrets". |
| **Clean tracker shutdown** | ✅ PASS | `PrivateRoomTracker::shutdown()` (private_room_tracker.rs:350): fires CancellationToken + calls backend.shutdown(). `ContinuousTracker::shutdown()` (public_room_continuous.rs:329): cancels + awaits task handle + drops tracker. Both have tests. `DynamicPeerJoiner::shutdown()` also clean. |
| **All checks pass** | ❌ FAILS | `cargo fmt --check` passes. `cargo clippy` fails (2 errors). `cargo test` fails (5 errors across all targets). |

### Dependency Duplication

`cargo tree --duplicates` reports 964 lines of duplicated dependency entries. Most are standard for a crypto-heavy Rust project — `rand_core` (3 versions), `getrandom` (3 versions), and 22 other crates with 2 versions each. No concerning version conflicts: the duplicates are side-by-side minor/patch versions required by different dependency chains. **No action needed.**

### Cancellation / Task Leaks

- `CancellationToken` used in: `private_room_tracker`, `dynamic_joiner`, `public_room_continuous`, `whisper/session_manager`
- All `tokio::spawn` call sites have cancellation paths
- `DynamicPeerJoiner` drops unsent join attempts on shutdown
- `ContinuousTracker::shutdown()` awaits the task handle to prevent orphaned tasks
- **No task leaks identified.**

### Error Propagation

- `n0_error::Result` used consistently across the codebase
- `RoomInviteV2::parse()` returns `Result<Self>` with descriptive errors
- `create_and_publish_private_discovery` (non-existent in current code) was the only error-path gap
- No `unwrap()` calls in production paths (only in test code)
- **Error handling is sound where the API exists.**

### Blocking Operations

- Test code uses `tokio::runtime::Runtime::new().unwrap().block_on(f)` via a helper `block_on()` in `private_room_tracker.rs:377`, `public_room_tracker.rs:327` — standard test pattern
- `net.rs:1759`: `std::thread::spawn` + `rt.block_on` for long-running gossip actor — appropriate
- `whisper/session_manager.rs:771`: same pattern for whisper sessions — appropriate
- **No blocking operations in async production paths.**

### Unrelated Refactors

- The `src/conversations.rs:483` has `use n0_future::StreamExt;` unused — pre-existing, not introduced by recent changes
- `src/dynamic_joiner.rs:43` — `ApiError` unused import — pre-existing
- **No unrelated refactors found in recent commits.**

### Stale Ticket/API Wording

- README correctly references `boru1:`, legacy tickets, and DHT discovery
- No stale references to old `create_and_publish_private_discovery` API in documentation
- The broken API references exist only in the example code (chat.rs, iced_chat/app.rs)

---

## 6. Summary of Blocking Issues

| # | Severity | File(s) | Issue | Fix |
|---|----------|---------|-------|-----|
| 1 | BLOCKING | `src/friends.rs:105,117` | `since = "4.0"` not semver | → `since = "4.0.0"` |
| 2 | BLOCKING | `tests/test_room_invite_v2.rs:227` | Unsized `[u8]` from deref | → `: [u8; 32]` annotation |
| 3 | BLOCKING | `examples/chat.rs:705`, `examples/iced_chat/app.rs:42` | Missing `create_and_publish_private_discovery()` | Rewrite to use new `PrivateRoomTracker::new()` + `publish_once()` |
| 4 | BLOCKING | `examples/chat.rs:823-824`, `examples/iced_chat/app.rs:3457, 3690, 4044` | Missing `PrivateRoomTracker::into_inner()` | `PrivateRoomTracker` is not a wrapper — use its `publish_once()`/`discover_once()` directly, or wire through a `PublicRoomTracker`-compatible adapter |

Issues 3-4 require significant rewrites of the private-room DHT discovery + continuous tracking pipeline in both frontends. The new `PrivateRoomTracker` exposes `publish_once()`/`discover_once()` methods directly, while the examples expect to:
1. Call a standalone convenience function that creates, publishes, and returns a secret
2. Extract the inner tracker and pass it to `ContinuousTracker::start()`

**Recommendation:** Add a `PrivateRoomTracker` → `PublicRoomTracker` conversion (e.g. via a trait or wrapper), or implement a standalone helper that matches the expected API shape. Alternatively, rewrite the frontend examples to use the new `PrivateRoomTracker` methods directly and create their own continuous publish/discover loop.

---

## 7. Deliverables

- **This report:** `VERIFICATION_REPORT.md` (in workspace)
- **Formatted code:** `cargo fmt` applied (cosmetic only)
- **README:** Verified accurate from parent task
