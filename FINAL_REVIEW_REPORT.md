# Final Review Report — Multi-Image Chat Fix

Task: t_8587cc93
Canonical checkout reviewed: `/home/dan/.hermes/kanban/boards/iroh-gossip-chat/workspaces/t_3d4e68eb`
The assigned workspace `/home/dan/.hermes/kanban/boards/iroh-gossip-chat/workspaces/t_1256c805` is a shared report workspace, not a Rust checkout.

## Verdict

**NEEDS CHANGES / BLOCKED.** The burst queue and success-path GUI behavior are verified, and the screenshot assertion regression is fixed in the current canonical checkout. One blocking recovery defect remains: the current canonical `AppMessage::ErrorMsg` handler still returns `iced::Task::none()`, so a failed queued image download does not start the next queued download.

## Findings

### 1. Rapid ImageShare bursts — PASS

- `src/chat_core.rs` stores pending images in a `Vec` and appends each event instead of replacing a single slot.
- The 2-image and 5-image unit tests passed.
- The 3-image GUI burst test passed and reported all three images downloaded in order.

### 2. FIFO, non-blocking UI drain — BLOCKING FAILURE PATH

- `examples/iced_chat/app.rs` uses `VecDeque`, `pop_front()`, and asynchronous `iced::Task::perform`; normal draining is FIFO and non-blocking.
- Completion, duplicate-skip, and network-event paths chain `start_next_pending_image_download()`.
- **Current source still has** `examples/iced_chat/app.rs:4754-4757`:

  ```rust
  AppMessage::ErrorMsg(msg) => {
      self.push_system(msg);
      iced::Task::none()
  }
  ```

- `start_next_pending_image_download()` maps download errors to `AppMessage::ErrorMsg`; therefore a failed download reports an error but leaves later queued images stalled until unrelated network activity.
- Required fix: return `self.start_next_pending_image_download()` from this arm (or equivalent), then rerun the focused suite. The prior implementer handoff claims this fix exists in commit `6ccb9ff`, but that commit is not present in the reviewed checkout history and the current source still has the defect.

### 3. Local versus remote rendering — PASS by source inspection

`image_chat_kind(sender, local_public)` selects local semantics for self-sent images and remote semantics for other senders. Image handle creation clears retained image bytes while persisted lookup remains sender/storage scoped.

### 4. Failure observability and panic safety — PASS with finding 2 caveat

Download failures are converted into visible system error messages, and image-store save failures are represented through image error state. No new unwrap/panic was found in the inspected queue/download path; recovery after an error is the blocking defect above.

### 5. Memory and regression risk — PASS based on inspection and focused tests

The pending queue stores metadata rather than full payloads, image bytes are cleared after handle creation, and sequential draining avoids unbounded concurrent downloads. The current checkout also has unrelated uncommitted parent/worktree changes; these were not attributed to the multi-image fix.

## Exact commands and evidence

Run from `/home/dan/.hermes/kanban/boards/iroh-gossip-chat/workspaces/t_3d4e68eb`:

- `cargo test --lib handle_net_event_two_image_shares_both_pending -- --nocapture` — **PASS, 1/1**.
- `cargo test --lib handle_net_event_five_image_shares_all_pending -- --nocapture` — **PASS, 1/1**.
- `cargo test --features gui --test test_multi_image_burst -- --nocapture` — **PASS, 1/1**; three images queued and downloaded successfully.
- `cargo test --features gui --test test_image_iced_gui_flow -- --nocapture` — **PASS, 1/1**; exact Iced image send/download flow passed.
- `cargo test --features gui --test test_iced_chat_flow -- --nocapture` — **PASS, 1/1**; two-way chat flow passed.
- `cargo test --features gui --test image_optimizer_integration test_screenshot -- --exact --nocapture` — **PASS, 1/1**; current assertion expects the intentional 1280px cap.

Warnings were limited to existing unused test imports/variables, the unused `iroh-dns` patch warning, and endpoint teardown logs. They did not fail the focused tests.

## Final assessment

The multi-image queue is correct for burst retention, FIFO success-path draining, rendering ownership, failure visibility, and memory behavior. Acceptance remains blocked until the current canonical source actually contains the ErrorMsg failure-path drain chain and the focused tests are rerun against that same checkout.
