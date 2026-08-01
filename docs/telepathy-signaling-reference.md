# Telepathy call signaling — reference pattern

Status: reference. This document distills the call-signaling protocol from
[chanderlud/telepathy](https://github.com/chanderlud/telepathy)
(`rust/telepathy-core/src/internal/messages.rs` and friends) into a pattern
boru can reuse for future session-teardown and capability-negotiation features.
Telepathy is a Flutter + Rust + iroh voice chat app; its signaling is a clean,
small example of a typed call lifecycle on top of iroh QUIC streams.

## Why this pattern matters for boru

Boru already has several session-style protocols (whisper DMs, file transfer,
backfill) but none of them carries a *typed reason* for why a session ended.
Callers today infer teardown from `Connected`/`Disconnected` events or a bare
`error: String` in a progress event. Telepathy shows the next step: a canonical
wire enum for teardown reasons plus a single handshake round-trip for capability
negotiation. That combination makes failure handling deterministic on both ends.

## 1. Protocol flow diagram

Telepathy uses one QUIC bidirectional stream per call session with
length-delimited framing (max frame 8 MiB, ALPN `telepathy/session/1`).
Audio itself flows out-of-band over QUIC datagrams with a 4-byte sequence
header; the control stream only carries lifecycle and chat messages.

```
 CALLER                                             CALLEE
   │                                                   │
   │  open QUIC stream (ALPN telepathy/session/1)      │
   │──────────────────────────────────────────────────>│
   │                                                   │
   │  Hello {                                          │
   │    ringtone: Option<Vec<u8>>,                     │
   │    audio_header: AudioHeader,                     │
   │    room_hash: Option<u64>,                        │
   │  }                                                │
   │──────────────────────────────────────────────────>│
   │                                                   │
   │  slot acquire:                                    │
   │    room match → accept                            │
   │    wrong room → Reject                            │
   │    busy       → Busy                              │
   │    user prompt → accept / reject                  │
   │                                                   │
   │  ┌─────────────────── accept prompt ─────────────┐│
   │  │  HelloAck { audio_header }  (capabilities)    ││
   │  │  ────────────────────────────────────────────<││
   │  │  Reject                                       ││
   │  │  ────────────────────────────────────────────<││
   │  │  Busy                                         ││
   │  │  ────────────────────────────────────────────<││
   │  └───────────────────────────────────────────────┘│
   │                                                   │
   │  both sides now have local + remote AudioHeader   │
   │  codec_config = merge(local, remote)              │
   │                                                   │
   │  ░░░░░░░░░░░░░░░░░  audio streaming  ░░░░░░░░░░░░░│
   │  datagrams: [seq:4][payload]  keepalive every 10s │
   │  jitter buffer: 5-frame latency, 32-frame max     │
   │  ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░│
   │                                                   │
   │  Goodbye { reason: GoodbyeReason }                │
   │  ────────────────────────────────────────────────>│  (or vice versa)
   │                                                   │
   │  stream closes; both sides release the call slot  │
```

Key transitions:

- **Hello → HelloAck** completes the handshake. The caller sends its
  `AudioHeader`; the callee replies with its own. Each side validates the
  other's header with `AudioHeader::is_valid()` and sends `Reject` on an
  invalid one.
- **Busy** is a transient signal: the callee has an active direct call and
  cannot take another. The caller maps it to "peer is busy" copy and tears
  down its side without retrying.
- **Reject** is a deliberate decline (wrong room, user declined, invalid
  header). Both `Busy` and `Reject` end the negotiation immediately.
- **Simultaneous dial** (both sides sent `Hello`) is resolved by a peer-id
  tiebreaker: the lower peer-id yields and accepts the incoming `Hello` as if
  it were the callee, mirroring the `HelloAck` path.
- **Hello timeout**: `HELLO_TIMEOUT` is 10 s, extended by 10 s when the caller
  sends a custom ringtone (so the callee has time to download and play it).
- **Goodbye** is the only terminal message and always carries a
  `GoodbyeReason` (see section 2).
- **KeepAlive** on the control stream and datagram keepalives on the audio
  path keep NAT mappings and iroh sessions alive during silence.

Room calls add one field: `Hello { room_hash: Option<u64>, .. }`. A matching
`room_hash` admits the peer to the shared room transport; a mismatched hash is
`Reject`ed; after admission the room handshake loop forwards `Goodbye` reasons
from any member to all others via `RoomControl::Goodbye(reason)`.

## 2. GoodbyeReason pattern

```rust
// rust/telepathy-core/src/internal/messages.rs
pub enum GoodbyeReason {
    SessionStopped,     // local stop_session token cancelled
    AudioDeviceError,   // mic/speaker failure, cpal/device error
    Error,              // everything else that is a failure
    None,               // deliberate end, no failure (hang up)
}

impl From<&Error> for GoodbyeReason {
    fn from(error: &Error) -> Self {
        if error.is_session_stopped() {
            Self::SessionStopped
        } else if error.is_audio_error() {
            Self::AudioDeviceError
        } else {
            Self::Error
        }
    }
}
```

### Why structured reasons beat ad-hoc error strings

1. **A closed vocabulary is matchable.** The receiver can `match` a
   `GoodbyeReason` and branch deterministically (silent hangup vs. show a
   dialog vs. surface "device problem"). A `String` forces string comparison
   and drifts out of sync with the sender's intent.

2. **Wire stays canonical; UI copy lives in one place.** The enum is the wire
   vocabulary. Telepathy converts it to user-facing sentences in exactly one
   place — `CallEndMessage::from_goodbye_reason`:

   ```rust
   GoodbyeReason::None            => ""                      // silent hangup
   GoodbyeReason::SessionStopped  => "The session was stopped"
   GoodbyeReason::AudioDeviceError=> "Audio device error"
   GoodbyeReason::Error           => "The call ended unexpectedly"
   ```

   Internal `Error` wording (`error.to_string()`) never reaches the frontend.
   This is the same discipline boru already applies with
   `FileAccessErrorCode::as_str()` — the pattern extends naturally to teardown.

3. **Error → reason is a lossy, deliberate classification.** `From<&Error>`
   reduces the rich internal error enum to the small wire enum. It throws away
   detail the peer does not need (device names, OS errors) and keeps the detail
   the peer *does* need (session stopped vs. device failure vs. generic). If a
   richer reason is later required, the enum grows variants; the wire version
   still stays bounded.

4. **`None` is a first-class "not an error" variant.** Hanging up is not a
   failure, and encoding it as `Error::Ok`-style empty string or a `None`
   option is worse than a dedicated variant. `GoodbyeReason::None` lets the
   receiving UI suppress the failure dialog and play a normal hangup tone.

5. **Reasons flow through the whole lifecycle.** `RoomControl::Goodbye(reason)`
   and `ProtocolMessage::error_goodbye(&error)` show that a single reason type
   is reused from the innermost error boundary to the wire and back to the UI.
   There is no second ad-hoc path.

## 3. Audio capability negotiation

```rust
// rust/telepathy-core/src/internal/messages.rs
#[derive(Readable, Writable, Debug, Clone, Default)]
pub(crate) struct AudioHeader {
    pub(crate) sample_rate: u32,
    pub(crate) codec_enabled: bool,
    pub(crate) vbr: bool,            // variable bitrate
    pub(crate) residual_bits: f64,   // bits-per-sample quality parameter
}

impl AudioHeader {
    pub(crate) fn is_valid(&self) -> bool {
        self.sample_rate < 128_000
            && self.sample_rate > 8_000
            && self.residual_bits <= 8_f64
            && self.residual_bits >= 2_f64
    }
}
```

### Single round-trip, both directions

The header rides inside `Hello` (caller → callee) and `HelloAck`
(callee → caller). After the handshake each side holds both
`local_configuration` and `remote_configuration` in `EarlyCallState`, and the
effective codec is a **merge, not a copy**:

```rust
// rust/telepathy-core/src/internal/state.rs
pub(crate) fn codec_config(&self) -> (bool, bool, f32) {
    let codec_enabled =
        self.remote_configuration.codec_enabled || self.local_configuration.codec_enabled;
    let vbr = self.remote_configuration.vbr || self.local_configuration.vbr;
    let residual_bits = (self.remote_configuration.residual_bits as f32)
        .min(self.local_configuration.residual_bits as f32);
    (codec_enabled, vbr, residual_bits)
}
```

- **Union for feature flags** (`||`): if either side supports the codec or VBR,
  the session may use it.
- **Min for continuous parameters** (`min`): the session uses the lower
  quality/resolution bound, so the weaker side always controls the ceiling.
- **Validation before adoption**: `is_valid()` bounds sample rate to
  (8 kHz, 128 kHz) and residual bits to [2, 8], and an invalid remote header
  is rejected with `Reject` — never silently adopted.

The output pipeline then uses the *remote* sample rate (what the sender's
frames actually are) while the input pipeline uses the negotiated codec
config. This is why capability exchange happens before any audio flows: the
jitter buffer is constructed with the negotiated sample rate and would be
wrong if both sides assumed their own.

### Why this beats "just use my default"

Without negotiation, two clients with different defaults (44.1 kHz vs 48 kHz,
codec on vs off) produce garbled or dropped audio and neither side knows why.
Negotiation makes the incompatibility visible, bounded, and resolvable in one
round-trip — and it gives each side a hook to *refuse* (`Reject`) instead of
degrading silently.

## 4. Applicability to boru

Boru has no call feature yet, but it already runs several session-style
protocols that would benefit from the same two ideas (typed teardown reasons +
single-round-trip capability negotiation). Concrete adoption points:

### 4.1 Group call setup (future voice feature)

- Adopt the `Hello`/`HelloAck` shape verbatim on a new ALPN (e.g.
  `/boru-call/1`) over a QUIC bidirectional stream, with `AudioHeader`
  negotiated in the same round-trip.
- Reuse the `room_hash` concept: boru already has stable room identities via
  gossip topics and `RoomHistoryStore`; a call `room_hash` derived from the
  room's member set (as telepathy's `room_hash_for_peers` does) lets a caller
  signal "join my existing room call" vs "start a new one" in the same `Hello`.
- Model the slot guard on `CallSlot`/`IncomingSlotDecision`: boru's
  conversation/gossip topology already needs a per-room admission decision;
  a `RoomMatch`/`Busy`/`Reject` outcome enum keeps that decision local and
  testable.
- Handle simultaneous joins with the deterministic comparator boru's
  `SessionManager` already uses for whisper collisions (lower public-key bytes
  wins) — telepathy's simultaneous-dial tiebreaker is the same idea.

### 4.2 Whisper session teardown

Boru's `WhisperWireMessage` has `Text`, `Control`, `MailboxEnvelope`,
`MailboxAck` — but **no teardown message at all**. The frontend learns about
disconnect only via `WhisperEvent::Disconnected`, which carries no reason, and
`SessionManager` treats every drop as reconnectable. Adding a typed end message
would let whisper distinguish:

- deliberate local `StopSession` (user closed the DM) → peer sees "session
  ended", no reconnect;
- transport failure → peer sees "connection lost", reconnect as today;
- device/app error → peer sees a useful reason instead of silence.

Concretely: add a `WhisperWireMessage::End { reason: WhisperEndReason }`
mirroring `GoodbyeReason` (`Stopped`, `TransportError`, `Error`, `None`), and
map `SessionState` transitions to it. Keep the enum as the only wire
vocabulary; render user-facing text in the GUI layer.

### 4.3 File transfer cancel reasons

`blob_transfer.rs` today reports:

```rust
pub enum BlobTransferProgress {
    Started { .. },
    Progress { .. },
    Completed { .. },
    Failed { error: String },   // bare string
    Cancelled,                  // no reason at all
}
```

`Failed`'s `String` is exactly the ad-hoc pattern telepathy avoids, and
`Cancelled` cannot tell the UI *who* cancelled or *why*. The telepathy pattern
suggests:

```rust
pub enum TransferEndReason {
    UserCancelled,     // local user pressed cancel
    PeerCancelled,     // remote peer aborted (needs a wire message)
    Timeout,           // READ_TIMEOUT_SECS elapsed with no progress
    StorageError,      // disk/write failure
    HashMismatch,      // verification failed
    Error,             // generic fallback
}
```

The wire side already has a strong precedent: `FileAccessErrorCode` is a
stable `#[repr(u8)]` enum with snake_case strings — extend the same discipline
to the transfer-progress/end path. A `Cancelled { reason: TransferEndReason }`
variant keeps the enum matchable while preserving the reason.

### 4.4 General rule for boru

Anywhere boru ends a session with a bare string or a silent disconnect, apply
the two-step:

1. Define a small wire enum for the *reason* (bounded, serializable,
   versioned).
2. Convert to user-facing copy in exactly one place (GUI layer), never
   `error.to_string()` on the wire or in the UI.

## 5. Anti-patterns to avoid

1. **Bare string errors on the wire.** `Failed { error: String }` forces
   string matching, lets internal wording leak, and cannot be extended without
   breaking consumers. Use a bounded enum; keep human sentences out of the
   protocol.

2. **No capability negotiation ("just use my default").** Two peers with
   different defaults fail in ways neither can diagnose. Always exchange a
   capability header in the handshake, validate it (`is_valid()`), and merge
   with `||`/`min` semantics.

3. **Silently adopting an invalid remote parameter.** If the peer's header
   fails validation, telepathy sends `Reject` — it does not clamp and proceed.
   Clamping hides the mismatch and produces subtly wrong audio.

4. **Leaking internal error detail to the peer or UI.** `error.to_string()`
   contains device names, OS messages, and internal state. Telepathy converts
   once (`CallEndMessage::from_error` / `from_goodbye_reason`) and never lets
   raw wording cross that boundary. Boru's `FileAccessErrorCode` already does
   this correctly; the teardown paths should too.

5. **No handshake timeout.** A call that hangs waiting for `HelloAck` forever
   wedges the UI and the slot. Telepathy's `HELLO_TIMEOUT` (10 s, +10 s for
   ringtone download) bounds the wait and releases the slot on timeout.

6. **Silent teardown with no distinction between "ended" and "failed".**
   A bare `Disconnected` event conflates deliberate hangup with failure. The
   `GoodbyeReason::None` variant exists precisely so "ended normally" is a
   first-class state, not the absence of an error.

7. **Unbounded control frames.** Telepathy caps control frames at 8 MiB
   (`SESSION_MAX_FRAME_LENGTH`) and rejects oversized writes with a bounded
   size writer. Any boru session protocol that adds lifecycle messages should
   keep the same frame limit and reject early.

8. **One error path per subsystem.** If reasons are enum-typed at the wire
   layer but strings at the callback layer, the mapping is duplicated and
   drifts. Thread a single reason type from the error boundary to the wire and
   back to the UI (telepathy's `GoodbyeReason` is reused everywhere, including
   `RoomControl::Goodbye(reason)`).

## Source map

| Concept | Telepathy source |
|---|---|
| `ProtocolMessage` enum | `rust/telepathy-core/src/internal/messages.rs` |
| `GoodbyeReason` + `From<&Error>` | `rust/telepathy-core/src/internal/messages.rs`, `error.rs` |
| `AudioHeader` + `is_valid()` | `rust/telepathy-core/src/internal/messages.rs` |
| `EarlyCallState::codec_config()` merge | `rust/telepathy-core/src/internal/state.rs` |
| `CallEndMessage` UI copy mapping | `rust/telepathy-core/src/internal/error.rs` |
| Handshake, slot guard, timeouts | `rust/telepathy-core/src/internal/core.rs` (`negotiate_outgoing_call`, `negotiate_incoming_call`, `call_handshake`, `room_handshake`) |
| Constants (`HELLO_TIMEOUT`, `KEEP_ALIVE`, `ALPN`, `SESSION_MAX_FRAME_LENGTH`) | `rust/telepathy-core/src/internal.rs` |
| Audio datagram framing, jitter buffer | `rust/telepathy-core/src/internal/connections.rs` |
