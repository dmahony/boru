# VC-064 adaptive video release close-out

Date: 2026-09-04
Evidence source revision: `47d7307a` plus the VC-064 close-out fixes
Disposition: **HOLD / staged rollout only**

VC-064 closes the adaptive-video release gate without overstating evidence. The
first staged release is H.264-only behavior: `Auto` selects H.264 conservatively,
and `H264Only` remains the emergency switch. AV1 auto selection is not enabled by
this gate. It may be enabled only after the missing runtime-failure evidence and
feature-isolation correction are complete. Existing v1 H.264/Opus negotiation and
wire fixtures remain the downgrade path.

## Final feature matrix

`python3 scripts/check-release-feature-matrix.py` — **PASS** (`release feature
matrix: OK`). The authoritative matrix remains `docs/release-feature-matrix.toml`:

| Platform | Declared release features | VC-064 disposition |
|---|---|---|
| Linux x86_64 | `gui,video-playback` plus default `net,metrics,wgpu-renderer` | H.264 adaptive call path staged separately; native camera/audio release acceptance remains untested |
| Windows x86_64 | `gui,terminal,voice-calls,video-calls,screen-sharing` plus default features | Blocked pending packaged native runtime evidence |
| macOS arm64 | `gui` plus default features | Unsupported for calls in this release matrix |

The all-features check was attempted from DEBSRV after the VC-064 merge. It
initially exposed integration omissions in the merged call runtime and test
harness; those omissions were fixed in this close-out. The final focused build
and test results below are the evidence for the staged call path. The broader
`--all-targets --all-features` gate still has unrelated existing test-fixture
errors (`NetEvent::Message` initializers missing `backfilled`), so it is not
reported as a pass.

## Evidence and exact counts

All Cargo commands were run through `rb` on DEBSRV; no local Cargo build is used.

| Area | Exact command/result | Disposition |
|---|---|---|
| Capability matrix | `python3 scripts/check-release-feature-matrix.py` — `release feature matrix: OK` | PASS |
| AV1/H.264 negotiation and typed fallback | `rb test --locked --lib --features net,voice-calls,video-calls -- call::video::negotiation --nocapture` — **6 passed, 0 failed** | PASS for conservative policy |
| H.264 encode/fragment/reorder/decode | `rb test --locked --test call_video_integration --features video-calls -- --nocapture` — **2 passed, 0 failed** | PASS |
| Two-peer media delivery and v1 audio/video fallback | `timeout 240 rb test --locked --test call_video_e2e --features net,voice-calls,video-calls -- --nocapture` — **2 passed, 0 failed, 0 ignored, 0 measured** | PASS (synthetic/local test relay) |
| Impairment recovery | Prior VC-060 evidence: `rb test --locked --test call_video_impairment --features video-calls -- --nocapture` — **8 passed, 0 failed** | PASS |
| v1 protocol fixture boundary | Prior M4 evidence: `rb test --locked --test call_protocol_boundary --features net,voice-calls,video-calls -- --nocapture` — **3 passed, 0 failed** | PASS |
| Performance harness | `timeout 240 rb test --locked --test call_perf_measurement --features voice-calls,video-calls -- --nocapture` — **10 passed, 0 failed, 0 ignored, 0 measured** | PASS |
| Performance measurements | 24 encode samples (avg 12.362 ms, p95 24.109 ms), 4 packetize (avg 0.011 ms, p95 0.015 ms), 4 reassemble (avg 0.973 ms, p95 1.741 ms), 4 decode (avg 42.780 ms, p95 54.307 ms); all under their adaptive assertions | PASS |
| All-feature library/binary check | `rb check --locked --all-targets --all-features` — **FAIL**, unrelated existing `NetEvent::Message` fixture initializers omit `backfilled` in `tests/test_hostile_input.rs` | NOT PASS / recorded failure |

The performance command originally failed because its new `VideoEncoder::encode`
API returns a vector; VC-064 updates the harness to consume non-empty access units
while retaining one timing sample per attempted frame. It then passed with the
exact count above. No zero-match command is treated as a passing test.

## Direct and relay smoke boundary

The two-peer E2E passed actual encode, QUIC datagram transport, decode, frame
application, camera transitions, and bilateral cleanup. Its deterministic local
relay fixture is useful transport evidence, but it is not a production relay-only
smoke. A separately provisioned direct-path and relay-only packaged-platform run
was not available in this gate and is **UNTESTED/BLOCKED**, not PASS.

Likewise, no complete live AV1 encoder-failure injection with simultaneous live
Opus audio exists yet. Unit-level fallback evidence plus H.264/audio E2E does not
substitute for that scenario.

## Rollout and rollback

1. Stage the H.264 conservative path only (`Auto`, or force `H264Only` for an
   emergency deployment).
2. Do not turn on AV1 auto selection until voice-only feature compilation,
   production direct/relay smoke, and AV1-failure-plus-live-Opus E2E evidence are
   present.
3. If a staged H.264 call regresses, set the runtime rollout mode to
   `H264Only`, preserve the existing v1 downgrade, and stop promotion.
4. If the candidate still violates the release boundary, retain the prior signed
   release and do not promote this candidate. Do not delete or mutate existing
   v1 fixture data while rolling back.

## Deferred follow-ups and known limitations

- L1T3 scalability remains deferred; the current advertisement is L1T1.
- VP9 remains deferred and is not advertised or selected.
- AV1 auto rollout remains held behind the missing end-to-end failure evidence.
- The `voice-calls`-only feature combination still needs explicit isolation from
  video-module imports before it can be claimed as independently supported.
- Native camera unplug/resume and packaged Windows/macOS runtime gates remain
  platform/manual acceptance work; synthetic tests cannot close those gates.

This is a deliberate fail-closed close-out: the H.264 staged path is supported by
focused evidence, while unavailable or incomplete gates remain explicitly held.
