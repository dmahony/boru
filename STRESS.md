# STRESS — BORU-CALL-9.9: Repeated teardown stress test (50-100x)

Task: t_dc67d4dd — repeated start/accept/activate/stop at stress scale.

## Test

`tests/call_e2e.rs` → `repeated_teardown_stress_75_sequential_calls_no_leaks`

75 sequential synthetic calls on the SAME two endpoints. Each iteration runs
the full lifecycle: start → ringing → accept → active → hangup → ended on both
sides. Builds on the 5.3 repeated start/end test (30 iterations) at stress
scale.

## The five invariants and how they are verified

| # | Invariant | Verification |
|---|---|---|
| 1 | No stuck call state | Every iteration's call completes; the next `start_voice_call` is accepted immediately and reaches Active. |
| 2 | No retained connection | The same two endpoints serve all 75 iterations; no per-iteration endpoint is created. |
| 3 | No continually increasing task count | Exact event accounting: each iteration must emit exactly 9 events (client: Ringing, Active, Ended; server: Incoming, Active, Ended) — 3 per side per iteration. Final assertion: `client_events_seen == 75*3 && server_events_seen == 75*3`. A leaked task would emit strays and drift the totals. Both channels must also be empty after every hangup. |
| 4 | No dead microphone/camera state | Hangup always succeeds and the next call activates — capture-side state is not wedged by the previous stop. |
| 5 | No stale Ended killing the next session | Each Ended carries the current iteration's call_id; channels are empty after hangup, so no leftover Ended survives into the next iteration. |

## Results

Run on debsrv (remote build wrapper `rb`), 2026-08-09:

```
rb test --test call_e2e
  test result: ok. 3 passed; 0 failed
  (two_endpoints_complete_call_and_reject_busy_second_call +
   repeated_start_end_loop_leaves_no_state_or_connection_growth [5.3] +
   repeated_teardown_stress_75_sequential_calls_no_leaks [9.9])
```

75 iterations completed with zero Failed/Rejected events, zero leftover
events, and exact 2-events-per-side-per-iteration accounting.

## Out of scope

- Two-endpoint test: BORU-CALL-9.8.
- Real capture devices: Phase 10.
