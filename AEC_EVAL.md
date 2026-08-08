# AEC Evaluation — BORU-CALL-3.13 (separate milestone)

**Date:** 2026-08-08
**Repo:** `https://github.com/dmahony/boru` (origin/main @ e9c2ebae)
**Status:** **DEFERRED** — documented below, per task body: *"If the evaluation
concludes AEC should be deferred, that is an acceptable outcome — document
findings and leave the pipeline extensible (a clear insertion point in the
audio processing chain)."*

## Summary

Echo cancellation (AEC) is **deferred**. The Phase 3 voice milestone completes
without it: the baseline 1:1 voice flow (ring/accept/active, bidirectional
synthetic + device audio, mute, loss survival, re-call) is verified by the
BORU-CALL-3.14 acceptance harness, and AEC is explicitly out of scope for that
gate ("do NOT block basic voice calls on AEC"). Adding AEC now would require
(1) a fully-wired duplex worker loop (capture → encode → network → decode →
playback with a shared media clock), which lands with the Phase 6 GUI, and
(2) a real speaker-to-microphone feedback measurement, which requires real
machines (Phase 10). Neither exists yet. No AEC code is added; the pipeline is
left extensible at the documented insertion point.

## Why AEC matters (and when it will)

AEC removes the caller's own voice from the microphone signal when it comes
back through the speaker. It only matters when **both** capture and playback
run on the same device at the same time (speakerphone / hands-free calls).
Until that duplex path is exercised, adding AEC is speculative: it adds a
stateful DSP stage with a reference-signal timing contract that cannot be
validated headlessly.

## Candidate implementations

| Candidate | Notes |
|---|---|
| `webrtc-audio-processing` (AEC3) | The maintained WebRTC AEC3; production-grade echo path + delay estimation. Large dependency (C++/cargo-cpufeatures). Most credible option when duplex ships. |
| `speexdsp-sys` | Lighter classic AEC. Older algorithm; still maintained. |
| `nnnoiseless` (already a dep via 3.12) | Noise suppression only — **not** AEC. Does not use a reference signal and cannot cancel echo. |

## Reference-signal plumbing required (documented for the future)

AEC needs BOTH signals with aligned timing:

1. **Microphone capture signal** — exists: `src/call/device.rs`
   `open_capture_stream` → `AudioCaptureProducer` (bounded SPSC, lock-free,
   CPAL callback only pushes samples; worker drains). 48 kHz internal rate,
   960-sample (20 ms) frames (`src/call/frame.rs`).
2. **Speaker/reference signal** — the sample stream actually sent to the
   playback device, not the pre-encode media. Currently the receive path
   (`src/call/audio/receive.rs`) produces device-rate interleaved samples into
   `PlaybackRing`; the worker feeds the CPAL output callback. AEC must tap the
   **post-decode, post-resample** playback samples (the same values the device
   plays) with the same media clock as capture.
3. **Clock alignment** — capture and playback must be driven from the same
   worker loop so the AEC's internal delay estimate starts from a bounded
   offset instead of an unbounded drift. The current device layer opens
   capture and playback as independent CPAL streams; the Phase 6 audio worker
   must run both and expose a shared frame clock.

## Current pipeline state (why deferral is correct now)

- `src/call/device.rs` exposes CPAL capture/playback primitives with bounded
  lock-free queues — the device boundary exists.
- `src/call/audio.rs` (`AudioCaptureProducer`/`Consumer`) + `noise.rs` (3.12,
  RNNoise stage) + `codec.rs`/`jitter.rs`/`plc.rs`/`receive.rs`/`send.rs` make
  up the DSP chain.
- The full worker loop that pulls from capture, runs resample → (noise) →
  Opus, sends over QUIC, and feeds decoded audio to playback **is not yet
  wired as one duplex loop** — the 3.14 acceptance harness drives media via
  `AudioSender` directly on a synthetic channel. The GUI call screen and its
  audio worker (Phase 6) is the natural place for this wiring.
- No speaker-to-microphone feedback measurement is possible in the current
  headless/test-vector environment; the task body requires a *real* test
  before adding AEC.

## Deferral rationale (acceptance criterion: "clear deferral rationale")

1. Baseline voice must not be blocked on AEC (task scope). It is verified
   without it (3.14 acceptance, 10/10 steps).
2. AEC cannot be validated without the duplex worker loop (Phase 6) and real
   hardware feedback (Phase 10). Implementing it now would ship an unverified,
   unmeasurable DSP stage.
3. The one justifiable pre-work — keeping the DSP chain insertable — is
   already satisfied: `noise.rs` (3.12) established the worker-side stage
   pattern (`process_frame(&mut [f32])`, runtime-gated, off the CPAL callback
   thread, Send+Sync). AEC slots in beside it with the same contract.

## Insertion point (pipeline left extensible)

```text
send path:   capture (CPAL, push-only)
             → worker drain (AudioCaptureConsumer)
             → resample (format::StatefulResampler)
             → [noise suppression — 3.12, optional]   ← AEC inserts HERE
             → Opus encode (codec::OpusEncoder)
             → media datagram → QUIC
receive:     QUIC → jitter → Opus decode → resample → PlaybackRing → CPAL out
             AEC reference tap: post-decode playback samples (same clock)
```

Concretely, the future AEC stage is a `struct AecStage { reference: ... }`
with `fn process_frame(&mut self, mic: &mut [f32], reference: &[f32])` inserted
next to `NoiseSuppressor` in the worker-side chain — **before** the noise
suppressor (AEC first, then residual noise suppression is conventional) or
after per tuning. It must be runtime-gated (default off) exactly like the
noise stage so the baseline voice path is bit-identical when disabled. The
reference buffer is fed from the receive/playback worker with the shared
frame clock described above.

## Re-evaluation trigger

Re-open this evaluation when BOTH hold:
1. Phase 6 GUI call screen lands with a single duplex audio worker loop
   (shared capture/playback clock), AND
2. A real two-machine (or speakerphone-on-one-machine) echo path can be
   measured — i.e. Phase 10 test machines are available.

At that point: implement `AecStage` behind `voice-calls` (default off),
wire the reference tap, and run the speaker-to-microphone feedback test
required by the task body before enabling it by default.
