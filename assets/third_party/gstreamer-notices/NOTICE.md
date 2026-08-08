# Third-Party Notices — Bundled GStreamer Runtime

This package bundles the **GStreamer MSVC runtime** (version **1.24.13**,
`gstreamer-1.0-msvc-x86_64-1.24.13.msi` from gstreamer.freedesktop.org) so
inline video playback works on a fresh Windows machine with nothing else
installed.  Only the reviewed subset described below is shipped; see
`scripts/gstreamer-windows-runtime.txt` and
`scripts/gstreamer-windows-plugins.txt` for the exact file lists.

## Included components and their licenses

| Component | Version | License | License text |
|---|---|---|---|
| GStreamer core (gstreamer-1.0-0.dll, gstbase, gstapp, gstvideo, gstaudio, gstpbutils, gstcontroller, gsttag, gstnet, gstallocators, gstfft, gstriff, gstrtp, gstcodecparsers) | 1.24.13 | LGPL-2.1-or-later | `LGPL-2.1.txt` |
| GStreamer plugins-base (playback, app, typefind, videoconvertscale, audioconvert, audioresample, audiorate, videorate, volume, autodetect, autoconvert, vorbis, opus, coreelements) | 1.24.13 | LGPL-2.1-or-later | `LGPL-2.1.txt` |
| GStreamer plugins-good (isomp4, matroska, audioparsers) | 1.24.13 | LGPL-2.1-or-later | `LGPL-2.1.txt` |
| GStreamer plugins-bad (vpx, videoparsersbad, opusparse) | 1.24.13 | LGPL-2.1-or-later | `LGPL-2.1.txt` |
| GStreamer libav plugin (gstlibav) | 1.24.13 | LGPL-2.1-or-later | `LGPL-2.1.txt` |
| FFmpeg libraries (avcodec, avformat, avutil, swresample, avfilter) | 6.0.x (as built into GStreamer 1.24.13 runtime) | LGPL-2.1-or-later | `LGPL-2.1.txt` |
| GLib/GIO/GObject (glib-2.0-0.dll, gio-2.0-0.dll, gobject-2.0-0.dll, gmodule, gthread, ffi, pcre2) | as shipped in the GStreamer runtime | LGPL-2.1-or-later | `LGPL-2.1.txt` |
| libvorbis / libogg (vorbis codecs) | as shipped in the GStreamer runtime | BSD-3-Clause | `BSD-3-Clause.txt` |
| Opus (libopus, opus-0.dll) | as shipped in the GStreamer runtime | BSD-3-Clause | `BSD-3-Clause.txt` |
| bzip2 (bz2.dll) | as shipped in the GStreamer runtime | BSD-4-Clause (bzip2 license) | `bzip2.txt` |
| zlib (z-1.dll) | as shipped in the GStreamer runtime | Zlib | `Zlib.txt` |
| orc (orc-0.4-0.dll) | as shipped in the GStreamer runtime | BSD-2-Clause | `BSD-2-Clause.txt` |
| intl (intl-8.dll, gettext runtime) | as shipped in the GStreamer runtime | LGPL-2.1-or-later | `LGPL-2.1.txt` |

## FFmpeg / libav licensing and codec-patent obligations

The `gst-libav` plugin and the bundled FFmpeg libraries (`avcodec-60.dll`,
`avformat-60.dll`, `avutil-58.dll`, `swresample-4.dll`, `avfilter-9.dll`)
are distributed under the **LGPL-2.1-or-later** license.  The GStreamer
project builds the Windows runtime without `--enable-gpl`, so the GPL
portions of FFmpeg are not enabled in this build.

**Redistribution review:** before publishing a Windows release that ships
these binaries, review codec patent obligations for the jurisdictions you
distribute to (notably H.264/AVC, AAC, and MPEG-4 patents).  This bundle
contains no `gst-plugins-ugly` plugins and no patent-encumbered format
plugins (x264, openh264, AMR, DTS, MPEG-2 encoders, DVD/DTV plugins were
excluded during review).  VP8/VP9, Opus, Vorbis, and Theora are covered by
royalty-free patent licenses from their respective holders.

## How this bundle was built

1. `scripts/gst_windows_manifest.py` computes the `bin/*.dll` dependency
   closure of the curated plugin allowlist
   (`scripts/gstreamer-windows-plugins.txt`).
2. `scripts/package_windows.sh` stages `boru.exe`, the Papirus icon assets,
   `gstreamer/1.0/msvc_x86_64/{bin,lib/gstreamer-1.0,libexec}`, this
   notice directory, and the toolchain runtime DLLs, then zips the result.

The application never mutates PATH.  On Windows the runtime is discovered
relative to the executable
(`<exe_dir>/gstreamer/1.0/msvc_x86_64/bin/gst-inspect-1.0.exe`), matching
`docs/video-runtime-packaging.md`.
