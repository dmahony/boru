# Live call media: no recording

Boru's live voice/video call media is memory-only. The call media boundary does
not write PCM samples, Opus packets, H.264 access units, camera frames, or
decoded remote frames to disk.

## Boundary checklist

- [x] Audio capture (`src/call/audio.rs`) copies samples into a bounded in-memory
      ring buffer. The callback does not perform filesystem or network I/O.
- [x] Audio codec, jitter, PLC, receive, and send stages pass owned/in-memory
      buffers to the next stage.
- [x] Video capture (`src/call/video/capture.rs`) returns `CapturedFrame` values
      in memory and does not expose a file-backed capture path.
- [x] Video encode/packetize (`src/call/video/codec.rs` and `packet.rs`) keeps
      raw RGB and encoded H.264 bytes in owned buffers until the network sender
      consumes them.
- [x] Video reassembly/decode (`src/call/video/reassembly.rs`, `pipeline.rs`,
      and `codec.rs`) stores only bounded in-memory state and latest-frame slots.
- [x] Media datagrams (`src/call/media.rs`) are read from and written to the
      Iroh connection; datagram serialization is an in-memory wire buffer.
- [x] The call module contains no filesystem imports or file-write APIs.

`tests/no_recording.rs` is a source-level guard. It recursively audits every
Rust source file below `src/call` and fails if a filesystem module or known file
creation/write API is introduced into the media path. The guard is intentionally
limited to the call module: chat history, file sharing, and user-requested call
recording (if added in the future) are separate persistence features and are
outside this boundary.

This policy does not claim that the operating system, audio/video drivers, or
other application features cannot record data. It defines the Boru call media
pipeline boundary: call media is not persisted by the pipeline itself.
