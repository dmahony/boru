# Review Report — Multi-Image Chat Fix and Regression Coverage

Task: t_8587cc93
Date: 2026-07-13
Reviewed checkout: `/home/dan/.hermes/kanban/boards/iroh-gossip-chat/workspaces/t_3d4e68eb`
Reviewed branch: `t_83367b85`, HEAD `7d0285d`

## Verdict

**NEEDS CHANGES / BLOCKED.** Rapid-burst retention, FIFO success-path draining, local/remote rendering ownership, failure visibility, and memory behavior are acceptable based on source inspection and the focused tests below. However, the current checkout still has a blocking failure-recovery defect: `AppMessage::ErrorMsg` reports a failed image download but returns `iced::Task::none()`, so later queued images are not started until an unrelated network event arrives.

## Findings

### Blocking: failed queued image download stalls the queue

File: `examples/iced_chat/app.rs:4754-4757`

```rust
AppMessage::ErrorMsg(msg) => {
    self.push_system(msg);
    iced::Task::none()
}
```

`start_next_pending_image_download()` maps download errors to `AppMessage::ErrorMsg` (lines 1787 and 1797). The handler therefore makes the error observable, but it does not call `start_next_pending_image_download()` after consuming the failed item. Any later images remain in `pending_image` until another `NetEvent` happens to trigger the drain. This violates the requirement that the UI drain queued images without blocking after failures.

Required implementation change: return `self.start_next_pending_image_download()` from this arm (or equivalent recovery logic), then rerun the focused suite against the same checkout. No code was changed by this review.

### No additional blocking findings

- `src/chat_core.rs:495,709` uses `Vec` plus `push`, so rapid ImageShare events are retained rather than overwriting one slot.
- `examples/iced_chat/app.rs:743,1760` uses `VecDeque` plus `pop_front`; the async `iced::Task::perform` path is non-blocking and FIFO.
- Success (`ImageDownloaded`), duplicate-skip, and NetEvent paths chain the next pending download.
- `image_chat_kind` maps self-sent images to `ChatKind::Local` and other senders to `ChatKind::Remote`; storage lookup uses local identity for local entries and sender identity for remote entries.
- Image entries are constructed with no retained raw bytes after handle creation, and pending queue entries contain metadata only. Sequential draining avoids concurrent payload accumulation. The queue has no explicit count bound, but its per-entry metadata is small and it is actively drained.
- Download and save failures are represented as `ErrorMsg`/`image_error`; no new panic/unwrap was found in the inspected image queue path.

## Test evidence

All commands ran from `/home/dan/.hermes/kanban/boards/iroh-gossip-chat/workspaces/t_3d4e68eb`.

- `cargo test --lib handle_net_event_two_image_shares_both_pending -- --nocapture` — PASS (1/1).
- `cargo test --lib handle_net_event_five_image_shares_all_pending -- --nocapture` — PASS (1/1).
- `cargo test --features gui --test test_multi_image_burst -- --nocapture` — PASS (1/1). Three rapid ImageShare events were retained and downloaded in order; output reported 3 successful downloads and 3 image entries.
- `cargo test --features gui --test test_image_iced_gui_flow -- --nocapture` — PASS (1/1). Exact Iced send/download flow, including a second image via `add_path`, passed and bytes matched.
- `cargo test --features gui --test test_iced_chat_flow -- --nocapture` — PASS (1/1). Two-way chat flow passed.
- `cargo test --features gui --test image_optimizer_integration test_screenshot -- --exact --nocapture` — PASS (1/1). The intentional 1280px cap assertion passes in the current checkout.
- `cargo test --lib` — PASS (387/387).

Observed warnings were non-failing: an unused `iroh-dns` patch warning, unused test imports/variables, and endpoint teardown logs from integration tests.

## Diff scope / attribution

`git diff github/main...HEAD` shows six committed files (+891/-18), primarily blocked/muted peer filtering and public-room identity/discovery. The reviewed multi-image queue implementation is in the existing ancestry before this branch. The checkout also has unrelated uncommitted parent/worktree changes (compression refactor and public-room safety files); they were not attributed to the queue defect except where directly inspected above.

## Final assessment

Accept the burst and normal-drain behavior, but do not accept the overall fix until the ErrorMsg failure path chains the next queued image and the focused GUI/burst tests are rerun after that change.
