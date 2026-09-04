# Live-call performance and platform release gates

Status: release gate definition and Linux measurement harness
Owner: VC-062

This document is the fail-closed acceptance contract for voice/video calls. The
measurement test is `tests/call_perf_measurement.rs`; it uses the production
Opus/OpenH264 codecs, packetizer, reassembler, and live video pipeline with
deterministic synthetic media. It reports wall-clock samples using
`std::time::Instant` (a monotonic clock), and prints one machine-readable
`CALL-PERF` line per video stage.

## Performance gates

| Gate | Threshold | Evidence |
|---|---:|---|
| Video frame budget | 41.667 ms at 24 fps | `VIDEO_FRAME_BUDGET` |
| Capture/transport average | <70% of frame budget (29.167 ms) | `video_stage_timings_meet_adaptive_frame_budget` |
| Capture/transport p95 | <100% of frame budget | same test; nearest-rank p95 |
| Software decode average/p95 | <70% / <100% of a two-frame adaptive budget | same test; 360p colorspace conversion is explicitly budgeted separately |
| Outbound audio queue | <=2 frames in the normal capture/flush cycle; hard implementation cap is 4 | `AudioSender::queued_frames()` and `MAX_OUTBOUND_AUDIO_FRAMES` |
| Incomplete video reassembly | <=10 frames | `MAX_INCOMPLETE_VIDEO_FRAMES`; loss test and expiry assertion |
| Call shutdown | <=2 s bounded cleanup | `CALL_SHUTDOWN_TIMEOUT`; manager shutdown tests |
| Conversational audio latency | <250 ms first decoded frame | `conversational_end_to_end_latency` |

A gate is a failure when its assertion fails, when the test cannot run, or when
its output is missing. Hardware and scheduler details must be recorded with the
command output; synthetic results are not a claim of native camera or relay
performance.

Run the focused harness on DEBSRV (never compile this project locally):

```sh
rb test --locked --test call_perf_measurement \
  --features voice-calls,video-calls -- --nocapture
```

The expected test inventory is 10 tests, with an exact nonzero test count of 0
on a passing run. The command must be run once per gate attempt; preserve the
complete stdout in the release evidence record rather than copying rounded
numbers by hand.

## Linux feature gate

Required Linux all-features verification is:

```sh
rb check --locked --all-targets --all-features
rb test --locked --test call_perf_measurement \
  --features voice-calls,video-calls -- --nocapture
```

The call harness specifically covers 20 ms Opus frames, adaptive jitter,
non-accumulating audio/video queues, 640x360@24 video bitrate, packet loss and
reassembly expiry, loopback conversational latency, and encode/packetize/
reassemble/decode timing. Existing manager unit tests cover bounded shutdown;
the release record must include their exact passed/failed/ignored counts when
that suite is run.

## Native feature matrix

| Platform | Target | Release feature set | Call status |
|---|---|---|---|
| Linux x86_64 | `x86_64-unknown-linux-gnu` | `gui,video-playback` plus default `net,metrics,wgpu-renderer` | Measure on DEBSRV; native camera/audio acceptance required before promotion |
| Windows x86_64 | `x86_64-pc-windows-msvc` | `gui,terminal,voice-calls,video-calls,screen-sharing` plus default `net,metrics,wgpu-renderer` | Native packaged artifact and desktop run required; `video-playback` intentionally disabled |
| macOS arm64 | `aarch64-apple-darwin` | `gui` plus default `net,metrics,wgpu-renderer` | Native runner required; voice/video/screen-sharing are not in the declared macOS release set |

Cross-compilation, source inspection, or a Linux run does not satisfy a native
Windows/macOS gate. An unavailable host is recorded as UNTESTED/BLOCKED, not as
PASS or FAIL.

## Manual smoke matrix

Execute for each packaged platform where the feature set is declared. Record
caller/callee build IDs, transport, camera device, host OS, and result.

| Scenario | Transport | Steps | Required result |
|---|---|---|---|
| Direct call | direct endpoint | Call A -> B; answer; exchange voice and video; hang up from both sides | Bilateral media, no stale call UI, both routers cleanly shut down |
| Relay call | relay-only/network with direct path unavailable | Repeat the same call over relay | Media remains conversational; reconnect/hangup are bounded |
| Camera unplug | direct and relay | Start video, unplug/disable camera during an active call | Voice/control remain alive; video reports a visible unavailable state; no task leak |
| Camera resume | same call after unplug | Restore camera, re-enable video, wait for a keyframe | New frames become visible without restarting the call; old generation cannot overwrite it |

Each row must be tested in both directions where applicable. A synthetic test
pattern may validate codec and transport plumbing, but cannot replace the
camera unplug/resume rows.

## Evidence record

Record the exact command, commit SHA, date, host CPU/GPU/audio/video devices,
feature flags, and the complete test summary. Include `PASS`, `FAIL`, or
`UNTESTED/BLOCKED` per matrix row and preserve nonzero counts exactly (failed,
ignored, measured, filtered). Never report a release gate as passed solely from
an unexecuted command or from another platform's artifact.
