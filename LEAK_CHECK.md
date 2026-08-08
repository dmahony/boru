# LEAK_CHECK — BORU-CALL-5.3: Media resource leaks + repeated start/end

Task: t_b7b5bbd8 — verify no media resource leaks after hangup and add a
repeated start/end test.

## Resource inventory (what a call owns)

`CallRuntime` (src/call/manager.rs) owns every per-call resource:

| Resource | Field / mechanism | Released by |
|---|---|---|
| Cancellation signal | `cancellation: CancellationToken` | `shutdown()` calls `cancel()` |
| Media admission gate | `accepting_media: AtomicBool` | `shutdown()` stores `false` (no new datagrams enter) |
| Connection | `connection: Connection` | `shutdown()` calls `connection.close(...)` |
| Control reader task | `control_reader_task` | bounded join / abort |
| Control writer task | `control_writer_task` | bounded join / abort |
| Media reader task | `media_reader_task` | bounded join / abort |
| Microphone capture task | `audio_capture_task` | bounded join / abort |
| Microphone send task | `audio_send_task` | bounded join / abort |
| Audio output task | `audio_receive_task` | bounded join / abort |
| Camera capture task | `video_capture_task` | bounded join / abort |
| Encoder task | `video_send_task` | bounded join / abort |
| Decoder task | `video_receive_task` | bounded join / abort |

`terminate_call()` (the single terminal transition from BORU-CALL-5.2) removes
the `CallState` from the actor's `calls` map, then runs
`state.runtime.shutdown().await`, which:

1. cancels the token,
2. flips `accepting_media` to false,
3. closes the connection (also closes the control and media streams),
4. takes all nine task handles and joins them with a bounded deadline
   (`CALL_SHUTDOWN_TIMEOUT` = 2s), aborting any task that outlives the grace
   period.

Because the actor serializes every command, and `terminate_call` only removes
the state for the *matching generation*, a stale task can never transition a
later call incarnation. Call state is fully removed: `calls.is_empty()` after
termination, so the user can start a new call immediately (verified in
`terminate_call_emits_exactly_one_ended_event`, BORU-CALL-5.2).

## Repeated start/end test (this task)

### Unit: `shutdown_releases_every_resource_slot_and_closes_connection`

`src/call/manager.rs` test module. Populates ALL NINE task slots with tasks
that block forever on a oneshot, then runs `runtime.shutdown().await` and
asserts:

- shutdown completes within `2 * CALL_SHUTDOWN_TIMEOUT` (bounded join; it does
  not hang on wedged tasks),
- every task was stopped — each oneshot sender sees its receiver dropped
  (i.e. the task was joined or aborted, not leaked),
- the connection reports `close_reason().is_some()` (the connection was
  closed by shutdown).

This covers the "microphone released, camera released, audio output released,
encoder released, decoder released, connection removed, worker tasks stopped"
acceptance for every tracked resource slot.

### Integration: `repeated_start_end_loop_leaves_no_state_or_connection_growth`

`tests/call_e2e.rs`. The SAME two endpoints run a full
start -> ringing -> accept -> active -> hangup -> ended lifecycle 30 times
(`ITERATIONS = 30`), asserting per iteration:

- the next call is accepted immediately after the previous hangup (the user
  can call again right away — no stale actor state),
- exactly one `Ended` per side,
- both event channels are empty after the terminal `Ended` (no leftover
  events from a leaked call state or connection),
- no `Failed`/`Rejected` events (call ids match every iteration).

Media is intentionally not involved (wire-level test, mirrors
`tests/call_e2e.rs` existing test); real capture devices are Phase 10 scope.
Resource accounting here is: call-state cleanliness, channel drains, and
connection lifecycle reuse across 30 iterations.

## Results

Run on debsrv (remote build wrapper `rb`), 2026-08-09:

```
rb test --lib call::manager::tests
  test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 2008 filtered out
  (10 pre-existing + shutdown_releases_every_resource_slot_and_closes_connection)
rb test --test call_e2e
  test result: ok. 2 passed; 0 failed
  (two_endpoints_complete_call_and_reject_busy_second_call +
   repeated_start_end_loop_leaves_no_state_or_connection_growth)
rb test --test call_timeout
  test result: ok. 1 passed; 0 failed
```

### Leak found and fixed (BORU-CALL-5.3)

`CallRuntime::shutdown()` previously wrapped the per-task bounded join in a
second `tokio::time::timeout(CALL_SHUTDOWN_TIMEOUT, ...)`. With a wedged task
in the first slot, that task consumed the whole 2s deadline, the outer timeout
fired, and the join loop future was dropped — which **detaches** the remaining
`JoinHandle`s instead of aborting them. Their tasks kept running (resource
leak). The 5.2 test only populated one slot, so this was never exercised.

Fix (src/call/manager.rs): removed the outer timeout. Each loop iteration
already waits at most the time remaining until `deadline` (zero remaining
means abort immediately), so the loop as a whole is bounded by
`CALL_SHUTDOWN_TIMEOUT`; every task is now joined or aborted. The new
`shutdown_releases_every_resource_slot_and_closes_connection` test populates
ALL nine slots with wedged tasks and proves every one terminates and the
connection closes.

## Out of scope

- Full 50-100x stress loop: BORU-CALL-9.9 (child task t_dc67d4dd).
- Real device capture/playback: Phase 10.
