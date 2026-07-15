# Review Report — Multi-Image Chat Fix

Task: t_8587cc93
Reviewed checkout: `/home/dan/.hermes/kanban/boards/iroh-gossip-chat/workspaces/t_3d4e68eb`
Branch: `t_83367b85`, HEAD `7d0285d`

## Verdict

**BLOCKED / NEEDS CHANGES.** Burst retention, FIFO success-path draining, local/remote storage semantics, and focused image flows are acceptable. One blocking recovery-path defect remains: a failed queued image download is reported through `AppMessage::ErrorMsg`, but that handler does not start the next queued download.

## Findings

### Blocking — failed image download stalls the queue

`examples/iced_chat/app.rs:1759-1798` removes one item from `pending_image` and maps download errors to `AppMessage::ErrorMsg`. However, `examples/iced_chat/app.rs:4754-4757` currently does:

```rust
AppMessage::ErrorMsg(msg) => {
    self.push_system(msg);
    iced::Task::none()
}
```

Thus an error is observable, but remaining queued images are not drained until unrelated later activity (such as another network event). The success path and duplicate-skip path do chain `start_next_pending_image_download()` (`4690-4691` and subsequent `ImageDownloaded` handling). Required correction: return `self.start_next_pending_image_download()` from this image-download error path, or otherwise guarantee the next queued image is scheduled. The current generic ErrorMsg arm may also serve unrelated errors, so the implementer should preserve existing error display behavior and avoid unintentionally chaining unrelated work.

### Pass — rapid ImageShare retention and ordering

`src/chat_core.rs:493-495` uses `Vec<(String, MessageHash, PublicKey)>`; `set_pending_image` appends with `push()` at lines 709-710. Unit tests assert FIFO retention for 2 and 5 events. The 3-image GUI burst test also observed all three downloads successfully and in order.

### Pass — non-blocking FIFO success path

`examples/iced_chat/app.rs:1759-1799` uses `VecDeque::pop_front()` and `iced::Task::perform`, so downloads are asynchronous and serialized. Completion and duplicate-skip paths schedule the next item. The failure path is the exception noted above.

### Pass — local versus remote rendering/storage

`image_chat_kind` at `app.rs:1716-1722` maps self-sent images to `ChatKind::Local` and other senders to `ChatKind::Remote`. `entry_storage_user` at `1708-1713` uses the local public key for local entries and sender key for remote entries; image hydration/lookup uses that mapping.

### Pass — observability and panic safety, with recovery caveat

Download errors become visible `ErrorMsg` system messages (`app.rs:1787`, `4754-4756`); image-store save failures populate `image_error` (`4704-4711`). The inspected queue path introduces no new unwrap/panic. Recovery remains incomplete because the error handler does not continue the queue.

### Pass — memory/regression inspection

The pending queue stores metadata, not image payloads. `ChatEntry::image` clears retained bytes in the relevant constructor path, and persisted history skips raw image bytes. The queue is not explicitly capped, but it is drained serially and each entry is small; no new unbounded payload retention was found.

## Commands and evidence

All commands ran from the reviewed checkout.

- `cargo test --lib handle_net_event_two_image_shares_both_pending -- --nocapture` — PASS (1/1).
- `cargo test --lib handle_net_event_five_image_shares_all_pending -- --nocapture` — PASS (1/1).
- `cargo test --features gui --test test_multi_image_burst -- --nocapture` — PASS (1/1); output reported 3 images downloaded successfully and 3 image entries.
- `cargo test --features gui --test test_image_iced_gui_flow -- --nocapture` — PASS (1/1); exact send/download flow passed for two images.
- `cargo test --features gui --test test_iced_chat_flow -- --nocapture` — FAIL in `test_iced_chat_exact_flow` at `tests/test_iced_chat_flow.rs:253`; peers remained at zero neighbors. This matches the documented pre-existing network/environment flake and is unrelated to image queue changes.
- `cargo test --features gui --test image_optimizer_integration test_screenshot -- --exact --nocapture` — PASS (1/1); 1920x1080 fixture resized to 1280x720, consistent with current working-tree cap and assertion.

Warnings/errors observed were existing `iroh-dns` patch-unused warning, unused test imports/variables, and endpoint teardown logs. The iced chat flow failure is environmental/network setup, not a compile or image assertion failure.

## Required follow-up

Implement and test failure-path queue continuation, ideally with a focused regression test that injects a failed first download and verifies the second queued image is attempted and rendered. Re-run the two unit tests and focused GUI/image suite after the change.
