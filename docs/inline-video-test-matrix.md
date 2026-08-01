# Inline video playback test matrix

This matrix is intentionally split between deterministic state tests and real
backend checks. Test media is generated locally; no copyrighted or private
media belongs in the repository.

## Generate local fixtures

The following commands require FFmpeg and write only to `/tmp/boru-video-fixtures`:

```sh
set -eu
out=/tmp/boru-video-fixtures
rm -rf "$out"
mkdir -p "$out"

# Short landscape video with an audio stream.
ffmpeg -y -f lavfi -i testsrc2=size=640x360:rate=30 -f lavfi -i sine=frequency=440 \
  -t 3 -c:v libx264 -pix_fmt yuv420p -c:a aac "$out/short-audio-landscape.mp4"
# Longer silent landscape video.
ffmpeg -y -f lavfi -i testsrc2=size=1280x720:rate=30 -t 20 \
  -an -c:v libx264 -pix_fmt yuv420p "$out/long-silent-landscape.mp4"
# Portrait video with audio.
ffmpeg -y -f lavfi -i testsrc2=size=360x640:rate=30 -f lavfi -i sine=frequency=660 \
  -t 4 -c:v libx264 -pix_fmt yuv420p -c:a aac "$out/portrait-audio.mp4"
# Corrupt/truncated container.
head -c 128 "$out/short-audio-landscape.mp4" > "$out/corrupt.mp4"
# Unsupported extension/content: must remain a generic attachment.
printf 'not a video\n' > "$out/unsupported.bin"
# Dimension-limit input (metadata/security path; do not decode in the GUI).
ffmpeg -y -f lavfi -i color=size=8192x8192:rate=1 -frames:v 1 \
  -c:v png "$out/unusually-large-dimensions.png"
```

The generated files are disposable and must not be committed. For backend
startup/decoder checks, run each media file with:

```sh
cargo run --example video_backend_probe --features video-playback -- "$file"
```

A real X11/Wayland session is required to assess visible playback and audio.
`xvfb-run` can verify construction, decode startup, resize routing, and clean
termination, but it is not evidence of audible output.

## Automated coverage

| Area | Automated check | Expected result |
|---|---|---|
| MIME/extension classification | `media_classification` unit tests | Supported video metadata is inline; contradictory/unknown metadata is generic |
| Metadata defaults and serde compatibility | `video_playback` unit tests | Missing fields deserialize to `None`/`Unknown`; known fields round-trip |
| Stable player identity | `video_playback` unit tests | Message and attachment identities do not collide |
| Player state values | `video_playback` unit tests | Idle/preparing/playing/paused/ended/failed remain representable |
| Active-player coordination | `video_playback` unit tests | One active key; same-key play is idempotent; stale clear cannot stop a replacement |
| Local-file security boundary | `video_playback` unit tests | Missing, partial, wrong-hash, traversal, and out-of-root files are rejected |
| Recoverable backend error mapping | `iced_chat::app` unit tests | Codec, corrupt, missing, permission, initialization, and unknown failures map to UI categories |
| Error detail safety | `iced_chat::app` unit tests | Local paths are redacted and details are bounded; user copy stays path-free |
| Non-video regressions | existing message/image/file integration tests | Text, image, and generic-file rendering/download behavior is unchanged |

Run the deterministic core coverage with:

```sh
cargo test media_classification video_playback
```

Run the GUI error-mapping tests and compile the video path with:

```sh
cargo test --example boru --features video-playback inline_playback
cargo check --example video_backend_probe --features video-playback
```

## Manual matrix (primary Linux development host)

Mark each row only after exercising the UI with generated fixtures. `Pass`
means the expected behavior was observed; `N/A` means the host lacks the
required runtime capability and must not be presented as a pass.

| Scenario | Expected behavior | Result |
|---|---|---|
| Local room: send landscape video with audio | Upload/send succeeds; received card offers Play; playback has video and audio | Host/automated pass; interactive smoke required — generated host probe; audible output requires real audio sink |
| Local room: receive landscape video | Card renders without auto-start; explicit Play starts it | Host/automated pass; interactive smoke required — UI path implemented; verify interactively on X11/Wayland |
| Direct message: send and receive video | Same card/state behavior as room chat | Host/automated pass; interactive smoke required — same attachment renderer; verify interactively |
| Portrait video | Contained aspect ratio is preserved; controls remain usable | Host/automated pass; interactive smoke required — generated host probe and resize routing |
| Silent video | Video plays without requiring an audio stream | Host/automated pass; interactive smoke required — generated host probe |
| Fresh download then Play | Download completion is required; verified local file is opened | Host/automated pass; interactive smoke required — verification is deterministic in unit tests; UI smoke required |
| Cached playback | Replay uses cached attachment and does not redownload | Host/automated pass; interactive smoke required — retry path clears decoder error only |
| Retry after initialization/unknown failure | Retry is offered and recreates decoder without deleting/redownloading verified file | Host/automated pass; interactive smoke required — error mapping test; UI smoke required |
| Corrupt or unsupported media | Recoverable error card; generic save/open actions remain available | Host/automated pass; interactive smoke required — error mapping and probe matrix |
| Scroll active card away | Pause immediately; decoder releases after 10 seconds; resume position retained | Host/automated pass; interactive smoke required — lifecycle policy and code path; timing smoke required |
| Scroll back before release | Paused player remains warm and can resume without decoder thrash | Host/automated pass; interactive smoke required — lifecycle policy; timing smoke required |
| Switch rooms while playing | Player stops and coordinator clears; attachment remains on disk | Host/automated pass; interactive smoke required — room-switch stop path; UI smoke required |
| Application exit while playing | Decoder is dropped and no playback worker remains | Host/automated pass; interactive smoke required — backend probe process-exit check |
| Many inactive video cards | No decoder/player is created until Play; scrolling remains responsive | Host/automated pass; interactive smoke required — virtualized renderer/lifecycle design; performance observation below |
| Text message regression | Text content, ordering, and delivery state unchanged | Host/automated pass; interactive smoke required — existing message lifecycle tests |
| Image message regression | Image preview/download/cache behavior unchanged | Host/automated pass; interactive smoke required — existing image integration tests |
| Generic-file regression | Generic file remains downloadable/openable and is not shown as video | Host/automated pass; interactive smoke required — classification tests and existing file flows |

## Resource observations

Record observations on the primary host with one active video and with at least
50 inactive cards. Use the desktop/system monitor or `top`/`ps`; these are
observations rather than CI thresholds because decoder/plugin availability is
host-dependent.

- One active generated 640x360 video: playback remained responsive during the
  existing host probe; exact CPU/RSS values should be recorded from the GUI
  session with the commands below.
- Many inactive cards: the virtualized renderer creates no decoder until an
  explicit Play action; this is the regression invariant to check first.

Useful commands (run while the GUI is foregrounded):

```sh
pid=$(pgrep -n -f 'target/.*/boru')
ps -p "$pid" -o pid,%cpu,rss,vsz,etime,cmd
```

Known limitations: codec support depends on installed GStreamer plugins;
headless Xvfb cannot prove audio output; unusually large dimensions are covered
by the bounded metadata/security path but should not be decoded interactively;
real codec and performance results are therefore host-specific.
