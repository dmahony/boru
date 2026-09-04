# VC-M2 adaptive H.264 live end-to-end gate

Date: 2026-09-04
Branch: `wt/t_c216f3c9`
Baseline: `origin/main` at `d4d5feaf`, with VC-024/031/032/033/034/035 integrated from `c0d48f70`.

## Verdict

**RED / do not proceed to AV1.** The codec and bounded media pipeline pass their synthetic and unit gates, but the complete M2 gate is not green: the existing two-peer signalling E2E suite has reproducible failures, and no test currently proves media datagrams traversing an Iroh connection into the receiver's decoded-frame watch.

## Evidence

| Area | Command / test | Result |
|---|---|---|
| Feature compilation | `rb check --lib --features video-calls,voice-calls` | PASS |
| GUI + call compilation | `rb check --bin boru --features gui,video-calls,voice-calls` | PASS (existing warnings) |
| Call unit/adaptation/runtime suite | `rb test --lib --features video-calls,voice-calls -- call` | PASS: 176 passed, 0 failed |
| Synthetic H.264 | `rb test --test call_video_integration --features video-calls -- --nocapture` | PASS: 2 passed, 0 failed |
| Synthetic Opus/audio | `rb test --test call_audio_integration --features voice-calls,video-calls -- --nocapture` | PASS: 2 passed, 0 failed |
| Iroh two-peer signalling | `rb test --test call_e2e --features gui,video-calls,voice-calls -- --nocapture` | FAIL: 2 failed, 1 passed. `two_endpoints_complete_call_and_reject_busy_second_call` observed an unexpected event before client `Ended`; `repeated_teardown_stress_75_sequential_calls_no_leaks` failed at iteration 42 waiting for client `Active`. |
| Isolated signalling reproduction | same suite with `--exact two_endpoints_complete_call_and_reject_busy_second_call` | FAIL at the same client `Ended` assertion |
| Binary test target | `rb test --bin boru --features gui,video-calls,voice-calls -- calls` | PASS compile; no matching tests were selected |
| Formatting/diff hygiene | `git diff --check` | PASS |

## Requirement mapping

- Camera consent gate: PASS by `call::manager::tests::runtime_shutdown_closes_media_gate_and_bounded_abort_wedged_task` and related manager/session tests; media starts disabled before Active/consent.
- Encode → fragment → reorder → reassembly → decode: PASS by `synthetic_video_encode_fragment_reorder_reassemble_decode` (4 synthetic frames, dimensions preserved, bounded reassembly state).
- Bounded sender and audio priority: PASS through the `call` unit suite, including media-sender ordering/replacement and adaptation's exact Opus ladder.
- Real stats and applied adaptation: PASS at unit level through the call adaptation/stats tests; no live stats capture was available.
- Keyframe recovery: PASS at unit level through the bounded recovery tests; no live transport recovery test was available.
- Latest-frame Iced watches: PASS at compile/integration wiring level; no GUI runtime assertion consumed a decoded watch update.
- Two real/in-process Iroh peers carrying media: NOT PROVEN. `call_e2e` establishes two Iroh peers for signalling, while `call_video_integration` is transport-independent synthetic media.
- Audio priority under live congestion: NOT PROVEN over an Iroh connection; covered by deterministic sender tests only.
- Recovery and deterministic shutdown: shutdown passes in manager tests; repeated signalling teardown is not green because the E2E stress test failed.

## Follow-up required before reopening M2

1. Add a two-endpoint Iroh media-datagram test that sends packetizer output through `Connection::send_datagram`, feeds the receiver-side media reader/reassembly/decoder, and asserts the latest-frame watch changes.
2. Fix or update `tests/call_e2e.rs` event assertions to tolerate non-terminal events emitted during teardown, then rerun the isolated and 75-iteration scenarios.
3. Add live stats/adaptation assertions and a bounded audio-before-video congestion scenario to the same peer harness.
4. Exercise camera consent with a synthetic capture backend and verify camera-off/reconnect clears stale frame watches.
