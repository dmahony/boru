# Inline video rollout evidence

Inline playback remains an opt-in Cargo feature (`video-playback`). The default
build does not link the player backend, and a build with the feature enabled
still gates Play on startup GStreamer capability detection. This is deliberate:
the clean packaged-runtime check is not reproducible on every development host.

## Performance and lifecycle review

The chat renderer creates a static poster card for inactive video attachments;
it does not construct `iced_video_player::Video` until Play is pressed. There is
at most one active player. When the card leaves the existing overscanned chat
window it pauses immediately, retains only the current position, and releases
the decoder after ten seconds. Room navigation, explicit close, end-of-stream,
and application teardown use the same stop path.

The 250 ms playback refresh subscription is now created only after a decoder
has loaded. Preparing sessions therefore do not trigger periodic layout
invalidations, and playback failures clear the coordinator slot before leaving
the recoverable error card in place. This avoids stale-player lockout on retry
and prevents a failed preparation from causing background redraw work.

## Verification performed

Deterministic checks:

```text
cargo test --lib video_runtime
cargo test --example boru --features video-playback inline_playback
cargo check --example video_backend_probe --features video-playback
```

Clean packaged-runtime checks:

```text
./scripts/check_video_runtime.sh
```

On the clean Ubuntu 24.04 container used for release validation, the documented
GStreamer runtime packages were installed, `./scripts/check_video_runtime.sh`
reported GStreamer 1.24.2 / runtime available, and `gst-launch-1.0 -q playbin`
successfully played generated `sample.mp4`, `sample.webm`, and `sample.mkv`
fixtures through `fakesink` sinks. The release binary's no-runtime smoke is
wired to the per-user log file at `<data_dir>/logs/boru.log` and checks for the
fallback warning that keeps Download and external-open actions available.

For a release candidate, record CPU, RSS, thread count, and open file
descriptors for the GUI process before and after: 50 inactive cards, first
startup, steady playback, seek, rapid scroll, player switch, close, and process
exit. Use the commands in
`docs/inline-video-test-matrix.md`; do not treat a headless Xvfb run as proof
of audible output.

## Supported behaviour and limitations

The release statement and runtime/plugin policy are documented in
`docs/video-runtime-packaging.md`: MP4/M4V/MOV, WebM, and Matroska/MKV are the
declared containers, subject to the packaged codec plugins. Unsupported or
corrupt media produces a recoverable error while download and external-open
actions remain available. Playback never autostarts, only verified local files
enter the decoder, and only one player is active at a time.

Linux uses documented system GStreamer dependencies. Windows must bundle and
validate the reviewed runtime beside the executable. macOS is not a supported
release target yet. Rollback is limited to disabling/removing the
`video-playback` feature from the packaged build; text, image, and generic-file
attachments retain their existing paths.

## Manual release gate

Do not enable the feature in the default Cargo feature set until a clean
packaged-build matrix passes on each release target. Required evidence includes
MP4, WebM, and MKV playback, missing-runtime startup, removal of a required
plugin, room and direct-message flows, dark/light and narrow-window layouts,
scroll-away cleanup, and application exit cleanup.
