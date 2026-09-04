# M3 gate: version 2 configuration compatibility

Date: 2026-09-04
Branch: `wt/t_41666849`
Base: `origin/main` at `d4d5feaf`, merged prerequisite commits `f57b8d98` and `f6410c04`.

## Verdict

RED. The v2 wire values and local track state machine are bounded and tested, but the M3 compatibility gate is not green. A v1 peer remains on the v1 H.264 path, while the call actor does not yet negotiate `OfferV2`/`AcceptV2` (the handler intentionally ignores those variants). No real two-peer v2 configuration exchange is therefore proven.

## Evidence

| Area | Command / test | Result |
|---|---|---|
| v1 wire compatibility | `rb test --lib --features net,voice-calls,video-calls -- call::wire` | PASS: 18 passed, 0 failed |
| Frozen v1 fixtures / feature boundary | `rb test --test call_protocol_boundary --features video-calls -- --nocapture` | PASS: 3 passed, 0 failed |
| v2 video track state machine | `rb test --lib --features net,voice-calls,video-calls -- call::video::packet` | PASS: 6 passed, 0 failed |
| Call manager lifecycle / stale generation | `rb test --lib --features net,voice-calls,video-calls -- call::manager` | PASS: 13 passed, 0 failed |
| Synthetic H.264 packet path | `rb test --test call_video_integration --features video-calls -- --nocapture` | PASS: 2 passed, 0 failed |
| Feature compilation | `rb check --lib --features net,voice-calls,video-calls` | PASS (existing warnings) |
| Two-peer call signalling | `timeout 300 rb test --test call_e2e --features gui,video-calls,voice-calls -- --nocapture` | FAIL: 1 passed, 2 failed; busy-call teardown assertion failed and 75-call stress failed at iteration 42 waiting for client Active |

## Requirement mapping

- v1 peers remain fixed H.264 / no v2 bytes: PASS at the wire boundary. The canonical v1 fixture identity is unchanged; the feature-boundary test passes. The normal call start path sends `Hello` version 1 and v1 `Offer`, not v2 control variants.
- Bounded v2 configuration: PASS for decode-time bounds. `OfferV2`, `AcceptV2`, `VideoTrackConfig`, and `ReceiverReport` are validated before use; codec list, resolution, frame-rate, bitrate, counter, non-zero track-id, and keyframe interval limits are covered by unit tests.
- Acknowledged track generations: PASS locally. Configuration is staged, video admission requires the matching acknowledged track and a keyframe, old tracks and pre-keyframe deltas are rejected, and unacknowledged configuration expires after `TRACK_CONFIG_ACK_TIMEOUT`.
- Stale-track rejection: PASS locally through `reorder_before_config_and_late_old_track_are_rejected` and runtime admission checks.
- Receiver-report adaptation: PASS locally for delta reports and manager adaptation routing; no two-peer v2 report exchange is proven.
- Two-peer v2 negotiation and media exchange: NOT PROVEN. `handle_control` explicitly ignores `OfferV2` and `AcceptV2`; the existing `call_e2e` suite exercises v1 signalling and is not an M3 v2 harness.

## Follow-up required before GREEN

1. Implement the v2 offer/accept lifecycle in the call actor, including negotiated v2 state and explicit track configuration exchange.
2. Add a two-endpoint test that sends v2 controls in both directions, observes the configuration acknowledgement, sends a keyframe plus delta, rejects an old track, and verifies receiver-report adaptation.
3. Resolve the existing `call_e2e` teardown/stress failures and rerun the two-peer gate with nonzero matches.
