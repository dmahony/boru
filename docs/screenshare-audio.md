# Screen-share system audio (BORU-SS-37)

System audio sharing lets the host stream what is playing on its machine to the
viewer during a screen-share session (RustDesk-style). Audio is a SEPARATE,
opt-in capability (`Capability::Audio`) — it is never enabled automatically
with the share, mirroring clipboard sync (PDF Task 9.3 / BORU-SS-25).

## How it works

- The host toggles audio with `HostCommand::SetAudioEnabled(bool)`
  (app message `ScreenShareToggleAudio`). Enabling grants the `Audio`
  capability to the viewer (the consent record), then starts a capture
  backend thread that pushes interleaved f32 PCM into a bounded
  `rtrb` ring (`audio_sample_ring`, 2 s capacity).
- The host streaming loop drains one 20 ms Opus frame per audio tick,
  encodes it (48 kHz stereo, `OpusAudioEncoder`), and sends it on the
  dedicated `AUDIO_KIND` QUIC stream (`ControlOut::Audio` →
  `QuicScreenTransport::send_audio`). `try_send` (drop-on-full) means a slow
  network drops audio, never blocks video.
- The viewer's protocol forwards `ReadUnit::Audio` to the app-facing audio
  channel ONLY when the session holds an explicit `Audio` grant
  (`end_to_end_audio_packet_delivery_is_grant_gated` covers the gate).
  Unauthorized audio is dropped without logging payload contents.
- The viewer's `audio_worker` decodes Opus (`OpusAudioDecoder`) and plays
  through `AudioOutput` (cpal). No output device (headless) → typed
  `AudioUnavailable` error, viewer continues view-only.

## Platform availability

| Platform | Capture backend | Playback | Status |
| --- | --- | --- | --- |
| Linux (PipeWire) | `PipeWireAudioCapture` (dlopen libpipewire, loopback input stream on the default audio sink, F32 48 kHz stereo) | cpal (ALSA/PulseAudio/PipeWire) | Implemented |
| Linux (no PipeWire runtime) | Typed unavailable error, session continues view-only | cpal | Implemented fallback |
| Windows | Not implemented — typed unavailable error (`UnavailableAudioCapture`) | cpal (WASAPI) | Stub |
| macOS | Not implemented — typed unavailable error | cpal (CoreAudio) | Stub |

The typed unavailable path is first-class: capture failure never fails the
video session; the sharer sees a toast ("System audio unavailable — …") via
`SessionEvent::AudioState { enabled: false, error }`.

## Wire format

- Stream kind byte `0x04` (`AUDIO_KIND`), distinct from control (`0x01`),
  media (`0x02`) and versioned messages (`0x03`).
- `AudioHeader` (postcard): version, session id (16 B), sequence (u64,
  non-zero), timestamp_us (u64), sample_rate (u32, 8000–192000), channels
  (u16, 1–2), payload_len (u32). Payload follows the header, bounded by
  `MAX_AUDIO_FRAME` (4096 B; Opus packets are ≤ 1275 B per RFC 6716, the
  extra headroom keeps untrusted peer input bounded).
- Encoded with `encode_audio` / decoded with `decode_audio` (transport.rs);
  the versioned `ScreenShareMessage::AudioPacket` carries the same fields
  postcard-encoded and is used for negotiation/tests.

## Codec profile

Opus (RFC 6716), 48 kHz, stereo, 20 ms frames (960 samples/channel),
`Application::Audio`, VBR, ~96 kbps. Frame size constants live in
`src/screen_share/audio.rs` (`AUDIO_SAMPLE_RATE`, `AUDIO_CHANNELS`,
`AUDIO_FRAME_MS`, `AUDIO_SAMPLES_PER_FRAME`).

## Known gaps (v1)

- Playback uses the device's default output config; if the device rate differs
  from the 48 kHz wire rate, audio plays at device rate without resampling.
- The PipeWire backend requests 48 kHz; if PipeWire negotiates a different
  rate the samples are still interpreted as 48 kHz (documented in
  `spawn_pipewire_loopback`).
- No A/V sync (audio timestamps are not used to schedule playback).
- Windows WASAPI loopback is the natural next backend.
