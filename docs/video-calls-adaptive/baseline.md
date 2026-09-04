# Adaptive low-bandwidth video calls: baseline (VC-001)

Status: frozen baseline; no production behaviour changes.

## Reproduction identity

- Starting commit: `d4d5feafc456edd50974a3a420800e434c8ff4c5`
  (`add architecture i18n and release candidate gates`).
- Worktree branch: `wt/t_fac87cff`.
- Snapshot platform: Linux 6.8.0-136-generic, x86_64 (Ubuntu host `7070`).
- Rust toolchain: `rustc 1.97.1 (8bab26f4f 2026-07-14)`;
  Cargo 1.97.1 (c980f4866 2026-06-30).
- Relevant feature set: `video-calls` (which implies `voice-calls`), plus
  `net` through the package's feature graph. The production default remains
  `net,metrics,gui,wgpu-renderer` and does not enable `video-calls`.
- The implementation is under `src/call/`; this document intentionally does
  not alter the call-control v1 or media wire representation.

## Commands and observed results

Commands below were run from this worktree. Heavy Cargo compilation was run on
DEBSRV through `rb`, not locally.

| Command | Result |
|---|---|
| `cargo metadata --locked --no-deps` | exit 0 (Cargo emitted only the existing format-version warning) |
| `git diff --check` | exit 0 |
| `rb test --test call_video_integration --features video-calls -- --list` | exit 0; 2 tests, 0 benchmarks |
| `rb test --test call_perf_measurement --features video-calls -- --list` | exit 0; 8 tests, 0 benchmarks |
| `rb test --test call_e2e --features video-calls -- --list` | exit 0; 3 tests, 0 benchmarks |
| `rb test --test call_timeout --features voice-calls,video-calls -- --list` | exit 0; 1 listed test |
| `rb test --test call_logging_policy --features voice-calls,video-calls -- --list` | exit 0; 1 listed test |

The list commands compile the selected test binaries on DEBSRV before listing
matches. The call-timeout output also contained the existing five library
warnings (unused imports/private interface/dead field); these are not caused by
this documentation-only change.

The currently matched integration tests are:

- `call_video_integration`: 2 synthetic H.264 encode/fragment/reorder/
  reassemble/decode and wire-round-trip tests.
- `call_perf_measurement`: 8 synthetic voice/video performance, jitter,
  queue-bound, latency, and reassembly tests.
- `call_e2e`: 3 endpoint call-control lifecycle, busy rejection, repeated
  teardown, and 75-call stress tests.
- `call_timeout`: 1 unanswered-offer negotiation-timeout test.
- `call_logging_policy`: 1 source audit ensuring no logging in hot audio/video/
  media paths.

These are test listings, not a claim that the full runtime/device acceptance
matrix has passed. Camera, microphone, relay, and GUI/device validation are not
part of this frozen baseline.

## Current H.264/video constants

Source of truth: `src/call/video/codec.rs`, `packet.rs`, `reassembly.rs`, and
`media.rs`.

| Constant / value | Current baseline |
|---|---:|
| Profile dimensions | 640 x 360 |
| Capture/encode frame rate | 24 fps |
| OpenH264 target bitrate | 600,000 bps (600 kbps) |
| Periodic keyframe interval | 48 encoded frames (2 seconds at 24 fps) |
| Live packet payload bound | 256 KiB (`MAX_VIDEO_PAYLOAD_BYTES`) |
| Encoded access-unit bound | 2 MiB (`MAX_ENCODED_VIDEO_FRAME_BYTES`) |
| Maximum fragments/access unit | 2048 |
| Incomplete reassembly frames | 10 |
| Reassembly deadline | 200 ms |
| Media header | 40 bytes, wire version 1 (`BCL1`) |
| Media kinds | Audio = 1, Video = 2 |
| Call-control ALPN | `/boru-call/1` |
| Call-control version/frame limit | v1 / 64 KiB |

Raw frames must have non-zero even dimensions and exactly RGB8
`width * height * 3` bytes. Video frames are fragmented according to the
negotiated datagram size; no fixed Ethernet/QUIC MTU is assumed.

## Call-control and actor-scoped state

Call control is still v1 and remains separate from media. `CallKind::Voice`
enables audio only; `CallKind::Video` enables audio and video. The call actor
owns call state in `src/call/manager.rs` and routes one media reader per call
connection. The actor-level maps are currently:

- `calls: HashMap<CallId, CallState>`;
- `terminal_calls: HashSet<CallId>` for terminal-generation protection;
- `media_state: HashMap<CallId, (audio_muted, video_enabled)>`;
- one `CallStatsAccumulator` and one `AdaptationController` in the actor loop;
- per-call runtime task handles for control reader/writer, media reader, audio
  capture/send/receive, and video receive work.

Call generations protect cleanup from stale background tasks. Pending incoming
offers are bounded at 32. Command and event channels are each bounded at 256;
media reader routing uses a bounded channel of 32. This is the state ownership
boundary future adaptive work must preserve: adaptive configuration must be
keyed to the active call/session, never global UI state.

## Media slots and drop policy

The receive video pipeline (`src/call/video/pipeline.rs`) has a bounded
reassembler plus a single latest decoded frame. A newly decoded frame replaces
the previous `latest_frame`; replacing an unpresented frame increments the
pipeline's dropped-frame counter. `VideoFrameSlots` has exactly two latest-value
slots: `latest_local_frame` and `latest_remote_frame`; it is not a history
queue. The local pipeline mirrors the preview frame but sends the original
(unmirrored) RGB data to the encoder. Disabled video produces no datagrams and
re-enabling requests a keyframe.

The reassembler keeps at most 10 incomplete frames and retires completed/
expired keys so delayed fragments cannot resurrect old frames. Duplicate
fragments are ignored. Overload therefore drops/rejects rather than accumulating
latency or unbounded memory.

## Current estimates, ignored events, and gaps

- `CallStats::default()` starts RTT, estimated send bitrate, estimated receive
  bitrate, packet/frame counters, and loss/drop counters at zero. The audio
  jitter estimator starts with a 75 ms target but its measured estimate is zero
  until arrivals are observed. A zero bitrate/RTT is therefore “not measured”,
  not a measured zero-throughput network.
- `AdaptationController` currently starts at a 1280x720, 30 fps, 2,500 kbps
  video decision, then degrades video before audio (2-sample worsening and
  3-sample recovery hysteresis). This does not match the live OpenH264 camera
  profile's 640x360, 24 fps, 600 kbps defaults; reconciling those values is a
  future adaptive-video task, not part of VC-001.
- `MediaKind::Video` is ignored by the audio path and `MediaKind::Audio` is
  ignored by the video pipeline. Non-video datagrams passed to
  `LiveVideoPipeline::receive_parsed` return `Ok(None)`. Empty decoder input
  and decoder buffering likewise return no frame.
- Malformed media datagrams become `MediaReaderEvent::Malformed`; the single
  media reader remains alive for later packets. The call actor reports this as
  `CallEvent::MediaMalformed` rather than changing the call-control wire
  protocol.
- The call actor emits low-frequency stats once per second and adaptation-change
  events only when the decision changes; media hot paths intentionally have no
  per-packet logs (enforced by `call_logging_policy`).
- The current repository does not provide a production camera-to-call GUI
  acceptance path or a real low-bandwidth/relay measurement in this baseline.
  Device capture, negotiated per-peer constraints, bitrate feedback quality,
  and UI presentation of adaptive state remain explicit gaps.

## Reusable screen-share references

The existing screen-share subsystem is separate from `src/call/video` and must
not be treated as a wire-compatible implementation. Useful references for
future design are:

- `docs/screenshare-current-state.md`: module map and lifecycle inventory;
  documents bounded latest-frame queues, H.264 codec wrappers, keyframe recovery,
  and the capture/encode/transport/decode boundaries.
- `docs/screenshare-quality-presets.md`: path-derived LAN/relay quality
  ceilings, conservative downward clamping, gradual upward recovery, and
  viewer quality ceilings.
- `docs/screenshare-feature-review.md`: rationale and explicitly separate
  follow-up capability decisions.
- `docs/screenshare-test-matrix.md`: platform/network verification limits and
  the distinction between automated and hardware/manual evidence.
- `docs/screenshare-media-path-benchmark.md` and
  `docs/screenshare-encode-benchmark.md`: reusable measurement/reporting
  patterns (not baseline measurements for live calls).
- `docs/screenshare/completion-report.md`: completed subsystem gate and
  evidence boundaries; useful for distinguishing screen-share validation from
  live-call validation.

## Rollback

This baseline is documentation-only. To roll back the VC-001 change while
preserving production code, remove the single file
`docs/video-calls-adaptive/baseline.md` (or revert the focused VC-001 commit).
Do not revert unrelated commits or reset the worktree. Before applying any
future adaptive implementation, verify that the starting SHA and call/media
wire tests above still describe the checked-out tree; if they do not, create a
new baseline rather than editing this historical record.
