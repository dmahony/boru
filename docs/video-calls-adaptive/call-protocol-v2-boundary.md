# Call protocol compatibility boundary (VC-040)

Status: frozen v1 wire contract; v2 is reserved and is not enabled.

This document is the compatibility contract for the call protocol. Changes to
any item marked frozen require a new protocol version and a new fixture set.
The Rust serde definitions in `src/call/wire.rs` implement this contract; this
boundary is intentionally documented before adding any v2 serde types.

## Version and feature gating

- The reliable control protocol version is `CALL_CONTROL_VERSION = 1`.
- The media datagram header version is `MEDIA_VERSION = 1`, with magic `BCL1`.
- `CallControl` and its v1 wire types are available without `voice-calls` or
  `video-calls`. This permits a peer without native devices to decode Hello,
  reject a call, and send Hangup without compiling a media backend.
- `video-calls` implies `voice-calls` in Cargo. `video-calls` selects camera and
  H.264 support; `voice-calls` selects microphone/audio support.
- No v2 control variants, v2 media header, or v2 codec are enabled by either
  feature. Enabling a feature is not a protocol-version upgrade.
- A Hello carries the sender's version as data. The wire decoder accepts the
  value so that a semantic handler can return `RejectReason::UnsupportedVersion`.
  It must not interpret an unknown version as v1.

Old-peer fallback is therefore explicit: negotiate v1 only when both peers
advertise/support v1. A peer that only offers v2 (once v2 exists) is rejected;
a peer with no video feature falls back to voice if the call kind permits it.
There is no silent downgrade of a v2 message to a v1 enum variant.

## Frozen v1 reliable control schema

`CallControl` is a postcard enum. Variant discriminants are the zero-based enum
order below and are frozen:

0. `Hello { version: u16, call_id: CallId }`
1. `Offer { call_id, kind: CallKind, capabilities: MediaCapabilities }`
2. `Ringing { call_id }`
3. `Accept { call_id, selected: NegotiatedMedia }`
4. `Reject { call_id, reason: RejectReason }`
5. `Busy { call_id }`
6. `MediaState { call_id, audio_muted: bool, video_enabled: bool }`
7. `RequestKeyframe { call_id, track_id: u32 }`
8. `KeepAlive { call_id }`
9. `Reconnect { call_id, generation: u64 }`
10. `Hangup { call_id, reason: HangupReason }`

`CallId` is exactly 16 bytes. The v1 audio capability is Opus, with supported
sample rate 48,000 Hz, one channel, and 20 ms frames. The v1 video capability
is H.264, up to 1920x1080 at 30 fps. H.264's postcard enum discriminant is
exactly zero. Capability and negotiated values are checked against the bounds
in `src/call/bounds.rs` before use.

The reliable stream uses a four-byte big-endian payload length followed by the
postcard payload. The payload limit is 64 KiB; a decoder rejects an oversized
length before slicing or allocating. The length prefix is framing, not a
version negotiation mechanism.

## Frozen v1 media bytes

Media packets are hand-framed, not postcard encoded. The fixed header is 40
bytes:

| Offset | Size | Meaning |
|---:|---:|---|
| 0 | 4 | ASCII `BCL1` magic |
| 4 | 1 | media version (`1`) |
| 5 | 1 | kind (`1` audio, `2` video) |
| 6 | 2 | flags, big-endian (`KEYFRAME=1`, `DISCONTINUITY=2`) |
| 8 | 16 | CallId |
| 24 | 4 | track id, big-endian |
| 28 | 4 | sequence, big-endian |
| 32 | 4 | timestamp, big-endian |
| 36 | 2 | zero-based fragment index, big-endian |
| 38 | 2 | fragment count, big-endian |
| 40 | rest | encoded media bytes |

All integer fields are unsigned and big-endian. A v1 video access unit is at
most 2 MiB and has at most 2048 fragments. Fragment indexes must be below the
advertised count, and the count cannot be zero. These limits are checked before
copying peer-controlled payload data.

## v2 sequencing and state boundary

The existing v1 sequencing is deliberately simple: a call has one CallId;
control messages are ordered by the reliable stream; media sequences increase
per track; fragment indexes reconstruct one access unit; `generation` identifies
an intentional reconnect. A discontinuity or reconnect causes the receiver to
drop incomplete media for the old generation and request a fresh keyframe.

```text
Idle
  | local Call (v1-compatible peer)
  v
HelloSent <----> HelloReceived
  | compatible version + authorized
  v
Offered --> Ringing --> Accepted
  |                    |
  |                    +--> MediaActive -- MediaState/KeepAlive --> MediaActive
  |                                      |
  |                                      +-- reconnect --> Negotiating
  |                                      +-- keyframe request --> MediaActive
  +--> Rejected / Busy

Any state -- Hangup / transport loss --> Ended
Unknown version -- semantic gate --> Reject(UnsupportedVersion) --> Ended
```

A future v2 may add sequencing or capability fields, but it must use a new
version boundary and a new discriminant/schema namespace. It must not append
fields to v1 postcard structs: postcard does not reliably apply serde defaults
to truncated trailing fields. V1 decoding remains stable for old peers.

## Golden identity

`tests/fixtures/call_control_v1_postcard.hex` is the canonical byte fixture for
Hello, Offer, Accept, MediaState, RequestKeyframe, Hangup, and H.264. The
`call_protocol_boundary` integration test reconstructs those values and fails
if any enum order, struct field order, or codec discriminant changes.
