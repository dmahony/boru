# Inline video backend probe

Step 8 uses `iced_video_player` **0.6.0**, which is built on GStreamer 1.x and
targets Iced 0.14. The dependency is optional and only enabled by the
`video-playback` feature; it is not part of normal Boru navigation or builds.

## Run the developer-only probe

```text
cargo run --example video_backend_probe --features video-playback -- /absolute/path/to/video.mp4
```

The probe is deliberately independent of chat messages. It opens the file on a
Tokio blocking worker, then sends backend status into the regular Iced update
loop. `VideoPlayer` emits `FramePresented`, `EndOfStream`, and `Error` events;
the probe maps them into `BackendEvent`. `Video` owns a GStreamer pipeline and
joins its worker during `Drop`, so dropping the app stops the pipeline and does
not leave a playback thread running.

Controls exercise pause/play, seek, mute, end-of-stream, and resize. Seek and
state changes are issued from `update`; decoding and frame delivery happen on
the player worker. Errors are shown in the status line rather than discarded.

## Runtime requirements

The Rust crate links against GStreamer development headers at build time. A
Debian/Ubuntu runtime needs at least:

```text
libgstreamer1.0-dev
gstreamer1.0-tools
gstreamer1.0-plugins-base
gstreamer1.0-plugins-good
gstreamer1.0-plugins-bad
gstreamer1.0-libav
```

`playbin`, `videoscale`, `videoconvert`, and an `appsink` are used by
`iced_video_player`; the actual decoder is selected by GStreamer. Therefore a
container opening is not sufficient evidence that a codec works: inspect the
probe's `Error` event and test the complete stream, including audio.

The probe matrix should include a video with audio, a silent video, a portrait
video, and an intentionally unsupported/corrupt file. In headless CI there is
no audio sink or graphics display, so use the build check and a real X11/Wayland
session for playback/audio verification. Do not treat a headless construction
test as proof of audio output.

The initial host matrix used generated local fixtures (`with-audio.mp4`,
`silent.mp4`, `portrait.mp4`, and an ASCII `unsupported.bin`). Each fixture was
run for five seconds under `xvfb-run`; all four probes stayed alive without a
GStreamer error before the timeout (exit 124 means the GUI was intentionally
terminated). This verifies construction, decoding startup, resize event
routing, and cleanup on process termination. Audio output itself remains a
manual check because Xvfb has no real audio sink.

## Current host verification

The build host was missing GStreamer headers/tools initially. Installing the
packages above provided GStreamer 1.24.2 development/runtime components and
FFmpeg/libav decoders. `cargo check --example video_backend_probe
--features video-playback` passes, as does the existing `video_playback` unit
test group (3 tests). The normal Boru GUI example also continues to compile
with `cargo check --example boru --features gui` (existing warnings only).