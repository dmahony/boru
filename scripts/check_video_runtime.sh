#!/usr/bin/env bash
# Validate the system GStreamer runtime used by Linux Boru packages.
set -euo pipefail

if ! command -v gst-inspect-1.0 >/dev/null 2>&1; then
  printf '%s\n' 'GStreamer runtime unavailable: install the documented GStreamer 1.x packages.' >&2
  exit 1
fi

missing=()
for element in playbin decodebin videoconvert videoscale appsink; do
  if ! gst-inspect-1.0 "$element" >/dev/null 2>&1; then
    missing+=("$element")
  fi
done

if ((${#missing[@]})); then
  printf 'GStreamer runtime incomplete; missing core elements: %s\n' "${missing[*]}" >&2
  exit 1
fi

gst-inspect-1.0 --version
printf '%s\n' 'Boru inline-video core runtime is available. Codec support still requires a real fixture playback test.'
