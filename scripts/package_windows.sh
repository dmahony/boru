#!/usr/bin/env bash
# package_windows.sh — assemble the self-contained Boru Windows package.
#
# Produces a zip containing everything the app needs at runtime on a fresh
# Windows machine with nothing else installed:
#
#   boru.exe
#   assets/third_party/papirus/          (icons — required by the runtime loader)
#   assets/emoji/twemoji/                (emoji SVGs + licence texts — required by the runtime loader)
#   gstreamer/1.0/msvc_x86_64/           (reviewed GStreamer runtime subset)
#   THIRD_PARTY_NOTICES/gstreamer/       (exact versions + license texts)
#   THIRD_PARTY_NOTICES.md               (full third-party notice manifest)
#   <toolchain runtime DLLs>             (mingw/llvm-mingw or VC++ redist)
#
# Policy source: docs/video-runtime-packaging.md.  Only the reviewed
# core/base/good/bad/libav plugin DLLs are bundled; gst-plugins-ugly and
# patent/format plugins are excluded until a separate legal review exists.
#
# Usage (bash; works on Linux for local verification and on the GitHub
# Actions windows-latest runner via git-bash):
#
#   scripts/package_windows.sh \
#       --exe <path to boru.exe> \
#       --gst-root <path to gstreamer/1.0/msvc_x86_64> \
#       --out <output dir> \
#       [--target x86_64-pc-windows-msvc|gnu|gnullvm] \
#       [--toolchain-dll-dir <dir containing runtime DLLs>]
#
# The script NEVER runs cargo. It only stages files and zips them.

set -euo pipefail

EXE=""
GST_ROOT=""
OUT_DIR=""
TARGET="x86_64-pc-windows-msvc"
TOOLCHAIN_DLL_DIR=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --exe) EXE="$2"; shift 2 ;;
        --gst-root) GST_ROOT="$2"; shift 2 ;;
        --out) OUT_DIR="$2"; shift 2 ;;
        --target) TARGET="$2"; shift 2 ;;
        --toolchain-dll-dir) TOOLCHAIN_DLL_DIR="$2"; shift 2 ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

[[ -n "$EXE" && -n "$GST_ROOT" && -n "$OUT_DIR" ]] || {
    echo "usage: package_windows.sh --exe <exe> --gst-root <gst_root> --out <dir> [--target <triple>] [--toolchain-dll-dir <dir>]" >&2
    exit 2
}

mkdir -p "$OUT_DIR"
STAGE="$OUT_DIR/stage"
rm -rf "$STAGE"
mkdir -p "$STAGE"

# ── 1. boru.exe ─────────────────────────────────────────────────────────
cp "$EXE" "$STAGE/boru.exe"
echo "[1/7] staged boru.exe ($(stat -c%s "$STAGE/boru.exe") bytes)"

# ── 2. Papirus icon assets (PAPIRUS-17, exe-relative loader) ────────────
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mkdir -p "$STAGE/assets/third_party"
cp -r "$REPO_ROOT/assets/third_party/papirus" "$STAGE/assets/third_party/papirus"
echo "[2/7] staged papirus icons"

# ── 2b. Twemoji emoji assets (BORU-TWEMOJI-23, exe-relative loader) ─────
# The emoji renderer probes <exe_dir>/assets/emoji/twemoji/svg/<key>.svg at
# runtime (src/bin/boru/emoji/renderer.rs).  Ship the whole vendored
# bundle — SVG artwork plus the verbatim upstream licence texts and the
# ATTRIBUTION.md that records the pinned revision (v15.1.0) — so a release
# artifact always carries the required third-party notices with the artwork.
mkdir -p "$STAGE/assets/emoji"
cp -r "$REPO_ROOT/assets/emoji/twemoji" "$STAGE/assets/emoji/twemoji"
echo "[2b/7] staged twemoji assets ($(find "$STAGE/assets/emoji/twemoji/svg" -name '*.svg' | wc -l) svgs + licence texts)"

# ── 3. GStreamer runtime subset ─────────────────────────────────────────
# Runtime DLLs (dependency closure) and curated plugin allowlist.  The
# manifest files below are generated from the reviewed GStreamer 1.24.13
# MSVC runtime and must be regenerated (scripts/gst_windows_manifest.py)
# if the pinned runtime version changes.
GST_BIN="$GST_ROOT/bin"
GST_PLUGINS="$GST_ROOT/lib/gstreamer-1.0"
GST_LIBEXEC="$GST_ROOT/libexec/gstreamer-1.0"
GST_DEST="$STAGE/gstreamer/1.0/msvc_x86_64"

BIN_MANIFEST="$REPO_ROOT/scripts/gstreamer-windows-runtime.txt"
PLUGIN_MANIFEST="$REPO_ROOT/scripts/gstreamer-windows-plugins.txt"

if [[ -f "$BIN_MANIFEST" && -f "$PLUGIN_MANIFEST" ]]; then
    mkdir -p "$GST_DEST/bin" "$GST_DEST/lib/gstreamer-1.0" "$GST_DEST/libexec/gstreamer-1.0"

    missing_bin=0
    while read -r f; do
        [[ -z "$f" || "$f" == \#* ]] && continue
        if [[ -f "$GST_BIN/$f" ]]; then
            cp "$GST_BIN/$f" "$GST_DEST/bin/$f"
        else
            echo "  WARN: missing runtime file $f" >&2; missing_bin=1
        fi
    done < "$BIN_MANIFEST"
    [[ $missing_bin -eq 0 ]] || { echo "FATAL: runtime manifest lists files absent from --gst-root" >&2; exit 1; }

    while read -r f; do
        [[ -z "$f" || "$f" == \#* ]] && continue
        if [[ -f "$GST_PLUGINS/$f" ]]; then
            cp "$GST_PLUGINS/$f" "$GST_DEST/lib/gstreamer-1.0/$f"
        else
            echo "  WARN: missing plugin $f" >&2; missing_bin=1
        fi
    done < "$PLUGIN_MANIFEST"
    [[ $missing_bin -eq 0 ]] || { echo "FATAL: plugin manifest lists files absent from --gst-root" >&2; exit 1; }

    # Tools needed by video_runtime detection and by GStreamer itself.
    cp "$GST_BIN/gst-inspect-1.0.exe" "$GST_DEST/bin/gst-inspect-1.0.exe"
    cp "$GST_BIN/gst-launch-1.0.exe"   "$GST_DEST/bin/gst-launch-1.0.exe"
    cp "$GST_LIBEXEC/gst-plugin-scanner.exe" "$GST_DEST/libexec/gstreamer-1.0/gst-plugin-scanner.exe"
    echo "[3/7] staged GStreamer runtime ($(find "$GST_DEST" -name '*.dll' | wc -l) dlls, $(find "$GST_DEST" -name '*.exe' | wc -l) tools)"
else
    echo "FATAL: runtime manifests are required ($BIN_MANIFEST / $PLUGIN_MANIFEST)" >&2
    exit 1
fi

# ── 4. THIRD_PARTY_NOTICES/gstreamer/ ───────────────────────────────────
NOTICES_SRC="$REPO_ROOT/assets/third_party/gstreamer-notices"
if [[ -d "$NOTICES_SRC" ]]; then
    mkdir -p "$STAGE/THIRD_PARTY_NOTICES/gstreamer"
    cp -r "$NOTICES_SRC/." "$STAGE/THIRD_PARTY_NOTICES/gstreamer/"
    echo "[4/7] staged third-party notices"
else
    echo "FATAL: GStreamer third-party notices are required at $NOTICES_SRC" >&2
    exit 1
fi

# Top-level notice manifest (BORU-TWEMOJI-23) — full inventory of every
# bundled/modified third-party component, incl. the Twemoji artwork entry.
if [[ -f "$REPO_ROOT/THIRD_PARTY_NOTICES.md" ]]; then
    cp "$REPO_ROOT/THIRD_PARTY_NOTICES.md" "$STAGE/THIRD_PARTY_NOTICES.md"
else
    echo "FATAL: THIRD_PARTY_NOTICES.md is required" >&2
    exit 1
fi

# ── 5. Toolchain runtime DLLs ───────────────────────────────────────────
# The exe's import table may reference toolchain runtime DLLs.  For GNU/
# llvm-mingw builds these live in the cross toolchain; for MSVC builds they
# are the VC++ redistributable (vcruntime140.dll etc.), normally found in
# System32 on the build runner.
copy_toolchain_dll() { # name, search dir
    local name="$1"; local dir="$2"
    if [[ -n "$dir" && -f "$dir/$name" ]]; then
        cp "$dir/$name" "$STAGE/$name"; return 0
    fi
    # Fall back to PATH / standard system locations.
    if command -v "$name" >/dev/null 2>&1; then
        cp "$(command -v "$name")" "$STAGE/$name"; return 0
    fi
    if [[ -f "/c/Windows/System32/$name" ]]; then
        cp "/c/Windows/System32/$name" "$STAGE/$name"; return 0
    fi
    return 1
}

case "$TARGET" in
    *-windows-gnu)
        # MinGW-w64 runtime DLLs
        for dll in libgcc_s_seh-1.dll libstdc++-6.dll libwinpthread-1.dll; do
            if copy_toolchain_dll "$dll" "$TOOLCHAIN_DLL_DIR"; then
                echo "  + $dll"
            else
                echo "  WARN: $dll not found (GNU build may not need it)" >&2
            fi
        done
        ;;
    *-windows-gnullvm)
        # llvm-mingw runtime DLLs
        for dll in libc++.dll libomp.dll libunwind.dll libwinpthread-1.dll; do
            if copy_toolchain_dll "$dll" "$TOOLCHAIN_DLL_DIR"; then
                echo "  + $dll"
            else
                echo "  WARN: $dll not found (gnullvm build may not need it)" >&2
            fi
        done
        ;;
    *-windows-msvc)
        # VC++ redistributable DLLs (dynamic CRT).  Statically-linked CRT
        # builds (crt-static) do not need these, but the default MSVC
        # release profile links the dynamic CRT.
        for dll in vcruntime140.dll vcruntime140_1.dll msvcp140.dll concrt140.dll; do
            if copy_toolchain_dll "$dll" "$TOOLCHAIN_DLL_DIR"; then
                echo "  + $dll"
            else
                echo "  INFO: $dll not found; dynamic CRT DLLs are OS/redist components" >&2
            fi
        done
        ;;
    *)
        echo "  WARN: unknown target '$TARGET'; skipping toolchain DLL step" >&2
        ;;
esac
echo "[5/7] staged toolchain runtime DLLs"

# ── 6. Zip + checksums ──────────────────────────────────────────────────
cd "$STAGE"
ARTIFACT_BASE="boru-windows-x86_64"
if [[ "$TARGET" == *aarch64* ]]; then
    ARTIFACT_BASE="boru-windows-aarch64"
fi

if command -v 7z >/dev/null 2>&1; then
    7z a -tzip "../$ARTIFACT_BASE.zip" . >/dev/null
elif command -v zip >/dev/null 2>&1; then
    zip -qr "../$ARTIFACT_BASE.zip" .
else
    echo "FATAL: neither 7z nor zip available" >&2; exit 1
fi

cd "$OUT_DIR"
sha256sum "$ARTIFACT_BASE.zip" > "$ARTIFACT_BASE.zip.sha256"
echo "[6/7] wrote $OUT_DIR/$ARTIFACT_BASE.zip"
echo
echo "=== package layout ==="
( cd "$STAGE" && find . -type f | sort )
echo
echo "=== checksums ==="
cat "$ARTIFACT_BASE.zip.sha256"
echo
echo "=== file count ==="
( cd "$STAGE" && find . -type f | wc -l )
