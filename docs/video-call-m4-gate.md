# M4 AV1 preference and H.264 fallback gate

Date: 2026-09-04
Commit under test: ae3e7197 (VC-054), combined with VC-053, VC-060, VC-061, and the M3 gate prerequisites.

## Gate evidence

| Area | Command | Result |
|---|---|---|
| AV1 preference, mixed capability fallback, runtime failure diagnostics, fresh-track policy, L1T1 advertisement | `rb test --locked --lib --features net,voice-calls,video-calls -- call::video::negotiation --nocapture` | 6 passed, 0 failed |
| Synthetic H.264 encode/fragment/reorder/reassemble/decode | `rb test --locked --test call_video_integration --features video-calls -- --nocapture` | 2 passed, 0 failed |
| Two-peer H.264 media encode/QUIC/decode/application and v1 audio/video fallback | `timeout 240 rb test --locked --test call_video_e2e --features net,voice-calls,video-calls -- --nocapture` | 2 passed, 0 failed |
| Impairment recovery, capacity ladder, audio priority, bounded queue, loss/reorder | `rb test --locked --test call_video_impairment --features video-calls -- --nocapture` | 8 passed, 0 failed |
| v1 fixture identity and feature selection | `rb test --locked --test call_protocol_boundary --features net,voice-calls,video-calls -- --nocapture` | 3 passed, 0 failed |
| Screen-share codec parity, AV1/H.264 round trips, keyframe and bitrate reconfiguration | `rb test --locked --lib --all-features -- screen_share --nocapture` | 318 passed, 0 failed, 6 ignored (324 matched) |
| Full feature compilation | `rb check --locked --all-features` | passed (existing warnings) |
| Audio round trip with video call feature set | `rb test --locked --test call_audio_integration --features net,voice-calls,video-calls -- --nocapture` | passed |

## Release hold

The M4 release gate is **HOLD**, not pass. The declared `voice-calls` feature matrix is currently not independently compilable: `rb test --locked --test call_audio_integration --features voice-calls -- --nocapture` fails before test execution because `src/call/adaptation.rs` and `src/call/media_sender.rs` unconditionally import the `video` module, which is gated behind `video-calls` (Rust E0433). This is an existing feature-isolation regression in the combined prerequisite work, and must be fixed before claiming uninterrupted audio in a voice-only build or proceeding to VC-064.

The runtime fallback unit path preserves the old AV1 track (`fallback_codec` returns H.264 without mutation), records the typed failure, and keeps the H.264 fallback on a new track. The two-peer E2E confirms actual H.264 media delivery and application; audio round-trip passes when the video-call feature set is enabled. No test currently exercises an AV1 call track plus a live Opus stream in the same network E2E, so that evidence remains unit-level/feature-level rather than a full AV1-failure runtime injection.

## Known limitations

- Calls conservatively select H.264 in `Auto`; AV1 is selected only by explicit `Av1Preferred` mode when all four encode/decode capability directions match.
- Advertised scalability remains L1T1; L1T3 and VP9 remain deferred.
- The all-feature check passes, but it does not prove the narrower `voice-calls` feature combination that currently fails compilation.
