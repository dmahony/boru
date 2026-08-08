# Voice Acceptance Test — BORU-CALL-3.14 (Phase 3 gate)

**Date:** 2026-08-08
**Repo:** `https://github.com/dmahony/boru` (origin/main @ 29699b40 + this task's harness)
**Harness:** `tests/voice_acceptance.rs` (headless, two iroh endpoints, actor/event level)
**Feature gates:** `net` + `voice-calls` (Opus codec, jitter buffer, AudioSender)

## Summary

All 10 Phase 3 voice acceptance steps PASS on two loopback iroh endpoints
(`presets::Minimal`, relay disabled). Full call cycle: ring → accept → active →
bidirectional synthetic audio → mute/unmute → loss burst survival → hangup →
immediate second call without restart.

**Result: PASS (1/1 harness test, 75/75 call-lib unit tests, 4/4 call
integration tests, full feature check clean).**

## Why the actor/event level (GUI Phase 6 not merged)

The task body permits actor-level validation: *"Where a GUI is needed, drive it
via the actor/CallHandle + events (no pixel automation). If the GUI (Phase 6)
is not yet merged, validate at the actor/event level with a headless driver and
document the mapping to the UI checklist."* The GUI call screen
(`CallHandle`/`CallEvent` wiring in `examples/iced_chat/main.rs`) is a Phase 6
deliverable and is NOT on main as of this run.

| UI checklist item (Phase 6+) | Actor/event mapping in this test |
|---|---|
| two instances become friends | two endpoints with known identities; direct CALL_ALPN connectivity (connect probe seeds the address cache, same as production mDNS/relay discovery) |
| open a direct chat | the direct connection between the two endpoints |
| click phone | `CallHandle::start_voice_call(peer)` |
| incoming call UI | `CallEvent::Incoming { kind: Voice }` |
| accept button | `CallHandle::accept(call_id)` → `CallEvent::Active` on both |
| talk | `AudioSender` (production opus datagram path) + `CallEvent::MediaReceived`, decoded non-silent |
| mute/unmute button | `CallHandle::set_muted` → `MediaStateChanged` both sides |
| hang up button | `CallHandle::hangup` → `CallEvent::Ended` both sides |
| call again | second `start_voice_call` on the same handles, no restart |

## Step-by-step results

| # | Step | Result | Evidence |
|---|---|---|---|
| 1 | two instances become friends | PASS | two endpoints with known public keys, direct probe connection established |
| 2 | open a direct chat | PASS | direct CALL_ALPN connection between endpoints |
| 3 | click phone | PASS | `start_voice_call` returns a call id; caller sees `OutgoingRinging` |
| 4 | receive incoming call event | PASS | callee sees `Incoming { call_id, kind: Voice }` with the SAME call id |
| 5 | accept | PASS | both sides see `Active { call_id }` |
| 6 | talk bidirectionally | PASS | 10 Opus-encoded sine frames caller→callee: all 10 `MediaReceived`, kind=Audio, call_id matches, first frame decodes to non-silent PCM (960 samples, 20 ms); 10 frames callee→caller likewise |
| 7 | mute/unmute | PASS | `set_muted(true)` → caller local `MediaStateChanged(audio_muted=true)` immediately (authoritative), callee sees the same via wire; `set_muted(false)` → both sides flip to false |
| 8 | survive temporary jitter/loss | PASS | sender dropped 3 of 12 frames (sequences 4–6 never handed to transport); call stayed Active, the 9 remaining frames arrived in order, dropped sequences confirmed absent |
| 9 | hang up from either side | PASS | callee hangs up → callee sees `Ended` (LocalHangup), caller sees `Ended` via wire |
| 10 | immediately make another call | PASS | second `start_voice_call` on the SAME handles: ring → incoming → accept → active → hangup. No restart required |

## Harness notes

- **Media channel stream-open pitfall:** the harness opens a dedicated media
  channel connection to the peer with `open_bi()`. quinn's `open_bi()` returns
  local stream handles immediately, but the remote's `accept_bi()` only
  completes when it receives an actual STREAM frame carrying the new stream id.
  A never-written stream is invisible to the remote, so the remote call actor
  never spawns its media reader and all datagrams are silently dropped. The
  harness writes one byte on the stream before sending audio.
- **Stream lifetime pitfall:** the bi-stream halves must be kept alive for the
  duration of the call. Dropping them sends a QUIC FIN; the remote wire session
  sees EOF, reports `ConnectionClosed`, and the actor ends every call to that
  peer with `ConnectionLost` — a spurious hangup mid-call.
- Loss simulation is sender-side (frames never handed to the transport), which
  exercises the "survive loss" criterion without netem. The jitter buffer's
  reorder/loss tolerance is separately unit-tested in
  `src/call/audio/jitter.rs`.
- The real app's audio path in Phase 6 will send over the call connection via
  the same `AudioSender`; the harness uses a dedicated channel because
  `CallHandle` does not expose the internal call connection.

## Verification commands (re-run)

```bash
rb test --features net,voice-calls --test voice_acceptance
rb test --features net --test call_e2e --test call_timeout
rb test --features voice-calls --test call_audio_integration
rb test --features net,voice-calls --lib call::
rb check --features gui,video-playback,terminal
```

All pass: 1 + 1 + 1 + 2 + 75 tests green, check exit 0.
