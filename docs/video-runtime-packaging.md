# Inline video runtime packaging

Inline playback is optional. A missing or incomplete GStreamer runtime must never
prevent Boru from starting; the attachment remains downloadable and can still be
opened with the operating system's external application.

## Declared support

The packaged runtime is required to support these containers/codecs:

| Format | Container/demuxer | Required decoder families |
|---|---|---|
| MP4/M4V/MOV | `qtdemux` | H.264/AVC and AAC (libav plugin), plus `videoconvert`/`videoscale` |
| WebM | `matroska`/`webmmux` demuxing | VP8, VP9, and Opus (good/bad plugin families) |
| Matroska/MKV | `matroskademux` | H.264/AVC or VP9 and AAC/Opus |

The UI may continue to classify other video-looking names as attachments, but
those files are not promised to play inline. Codec negotiation is validated by
opening the completed, verified local file; runtime presence alone cannot prove
that an individual file is decodable.

The core player elements checked at startup are `playbin`, `decodebin`,
`videoconvert`, `videoscale`, and `appsink`. They come from GStreamer core/base;
codec support comes from the `gst-plugins-good`, `gst-plugins-bad`, and
`gst-libav` families. Do not ship `gst-plugins-ugly` or patent/format plugins
without a separate legal and redistribution review.

## Deployment policy

* Windows release installers bundle the GStreamer MSVC runtime beside the Boru
  executable under `gstreamer/1.0/msvc_x86_64/`, including the runtime DLLs and
  only the reviewed core/base/good/bad/libav plugin DLLs. The installer must
  include the corresponding GStreamer, FFmpeg/libav, and plugin license notices.
* Linux packages use the distribution's GStreamer 1.x dependency set rather than
  copying system libraries into the application. The package dependency must
  include the core, tools, base, good, bad, and libav runtime families.
* macOS packaging is not currently a supported release target. A future bundle
  must ship and validate a signed GStreamer framework before enabling the video
  feature; development-machine availability is not release evidence.

The application does not mutate PATH. On Windows the runtime is found relative
to the executable; on Unix the documented `gst-inspect-1.0` system executable is
used. Startup detection records the exact missing core elements and playback is
gated when they are unavailable.

## Clean-environment checks

On Windows, install the release artifact in a fresh VM with no GStreamer SDK or
MSYS2 installation. Verify that `gst-inspect-1.0.exe` is found relative to Boru,
play one MP4, WebM, and MKV fixture, and confirm that removing one plugin DLL
shows a recoverable capability message while Download and Open remain usable.

On Linux, run the release artifact in a clean container/VM with only the package
dependencies, then repeat with the GStreamer runtime removed. The latter must
start successfully and disable only inline playback.

The helper `scripts/check_video_runtime.sh` performs the Linux prerequisite and
core-element checks; it intentionally exits non-zero when the runtime is absent
so CI/package jobs cannot silently advertise inline playback.

Current repository state: the Windows packaging pipeline is implemented
(2026-08-08).  `.github/workflows/release.yaml` downloads the pinned
GStreamer MSVC runtime MSI on the `windows-latest` runner and
`scripts/package_windows.sh` assembles the self-contained artifact:
`boru.exe`, the Papirus icon assets, the reviewed runtime subset under
`gstreamer/1.0/msvc_x86_64/`, `THIRD_PARTY_NOTICES/gstreamer/`, and the
toolchain runtime DLLs.  The curated plugin allowlist and the runtime DLL
closure are checked in as `scripts/gstreamer-windows-plugins.txt` and
`scripts/gstreamer-windows-runtime.txt`; regenerate with
`scripts/gst_windows_manifest.py` when the pinned runtime version changes.

The Windows release build uses `--features gui` (no `video-playback`) by
design (WIN-FEAT-01, direction b). gstreamer-sys links GStreamer through
import libraries, so enabling the feature would make `boru.exe` fail to
start whenever the runtime DLLs are not in the Windows loader search path —
contradicting the clean-start guarantee above. Instead, non-`video-playback`
builds (Windows) serve undownloaded videos over the built-in local HTTP
streaming server (`src/streaming_server.rs`) and open the URL in the OS
default player (VLC/browser); fully downloaded videos keep the Download and
Open actions, and `src/video_runtime.rs` capability detection is untouched.
The bundled runtime is still shipped so the package stays self-contained,
`gst-inspect-1.0.exe` detection keeps working, and the layout is stable for
a future native-CI build that moves inline playback to dynamic loading or
delay-load imports (direction a).

## Licensing and notices

Redistributed GStreamer and plugin binaries remain under their upstream licenses.
Release artifacts must ship `THIRD_PARTY_NOTICES/gstreamer/` with the exact
versions and license texts for every included DLL/plugin. FFmpeg/libav licensing
and codec patent obligations must be reviewed for the chosen build before a
Windows release is published.
