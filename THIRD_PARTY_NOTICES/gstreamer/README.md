# GStreamer and plugin notices

A release that bundles GStreamer must copy the exact license texts and version
manifest for every redistributed GStreamer, GLib, FFmpeg/libav, and plugin DLL
into this directory. Do not treat this README as a substitute for those notices.

The repository does not currently contain redistributed media binaries. Linux
packages depend on the system runtime, and Windows packaging must populate this
directory as part of the release build after legal review of the selected codec
plugins. See `docs/video-runtime-packaging.md`.
