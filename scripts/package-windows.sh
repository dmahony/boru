#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# package-windows.sh — build a Windows Boru exe on Linux and package it with
# the bundled Papirus icon assets, so file-type icons resolve at runtime.
#
# WHY THIS EXISTS (t_7c04a3ee / Windows file-type icon bug):
#   The Papirus icon bundle is NOT embedded in the binary (PAPIRUS-02
#   licensing gate: GPL-3.0 SVG bytes must not be embedded).  At runtime the
#   loader (`papirus_asset_root()` in examples/iced_chat/file_type_icon.rs)
#   resolves the bundle in priority order:
#     1. BORU_PAPIRUS_ASSETS env var
#     2. <exe_dir>/assets/third_party/papirus   ← release package layout
#     3. <exe_dir>/../assets/third_party/papirus
#     4. <cwd>/assets/third_party/papirus
#     5. CARGO_MANIFEST_DIR/assets/third_party/papirus (compile-time baked)
#   For a Windows exe CROSS-COMPILED on Linux, candidate 5 is the Linux
#   build-machine absolute path, which cannot exist on the Windows host.  A
#   bare exe with no assets tree therefore renders only the embedded generic
#   icon.  THIS SCRIPT ships the assets tree next to the exe (candidate 2),
#   the same layout `.github/workflows/release.yaml` packages for GitHub
#   releases.
#
# USAGE:
#   ./scripts/package-windows.sh            # debug build (faster iteration)
#   ./scripts/package-windows.sh --release  # release build (delivery)
#
# OUTPUT:
#   dist-windows/boru.exe
#   dist-windows/assets/third_party/papirus/...
#   boru-windows-x86_64.zip                 # exe + assets tree, like release.yaml
#
# Cross-compile prerequisites (Debian/Ubuntu):
#   rustup target add x86_64-pc-windows-gnu
#   sudo apt install gcc-mingw-w64-x86-64 g++-mingw-w64-x86-64
#   # The posix-thread variant of the mingw toolchain is REQUIRED for Rust
#   # cross-builds (win32-thread libstdc++ lacks std::mutex/std::thread).
#   dpkg -L g++-mingw-w64-x86-64 | grep -- '-posix$'   # verify posix variant
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
TARGET=x86_64-pc-windows-gnu
DIST="$ROOT/dist-windows"
MODE="${1:---debug}"

# The `gui` feature only — `video-playback`/`terminal` pull GStreamer/GTK
# system libs that do not cross-compile without a Windows sysroot.
FEATURES="gui"
EXAMPLE="boru"

echo "── Cross-building boru for $TARGET ($MODE, features=$FEATURES) ──"

if ! rustup target list --installed | grep -q "^$TARGET$"; then
    echo "ERROR: target $TARGET not installed. Run: rustup target add $TARGET" >&2
    exit 1
fi
if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    echo "ERROR: mingw linker missing. Run: sudo apt install gcc-mingw-w64-x86-64 g++-mingw-w64-x86-64" >&2
    exit 1
fi

# Posix-thread mingw is required for Rust cross-builds.  Prefer the explicit
# -posix binaries; fall back to the plain names if only those exist.
CC_POSIX="$(command -v x86_64-w64-mingw32-gcc-posix || command -v x86_64-w64-mingw32-gcc)"
CXX_POSIX="$(command -v x86_64-w64-mingw32-g++-posix || command -v x86_64-w64-mingw32-g++)"
AR_POSIX="$(command -v x86_64-w64-mingw32-gcc-ar-posix || command -v x86_64-w64-mingw32-ar)"
echo "mingw CC=$CC_POSIX"

export CC_x86_64_pc_windows_gnu="$CC_POSIX"
export CXX_x86_64_pc_windows_gnu="$CXX_POSIX"
export AR_x86_64_pc_windows_gnu="$AR_POSIX"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$CC_POSIX"
# NOTE (2026-08-08, t_7c04a3ee): the Debian 12 mingw-w64 headers installed
# with GCC 13 already declare `RelationProcessorDie` in winnt.h, so the old
# `-DRelationProcessorDie=((LOGICAL_PROCESSOR_RELATIONSHIP)5)` CXXFLAGS work-
# around from the rust-windows-build-delivery skill is NOT needed and in fact
# BREAKS the tracy build (the define collides with the enum in winnt.h).
# Verified: tracy-client-sys 0.28.0 compiles cleanly with no define.

if [ "$MODE" = "--release" ]; then
    PROFILE_FLAG="--release"
    PROFILE_DIR="release"
else
    PROFILE_FLAG=""
    PROFILE_DIR="debug"
fi

# Redirect (no pipe) so cargo's exit status is preserved — a pipeline would
# mask a failure and leave a stale exe (see the skill's pipe-mask pitfall).
mkdir -p "$ROOT/target"
cargo build $PROFILE_FLAG --target "$TARGET" --features "$FEATURES" --bin "$EXAMPLE" \
    > "$ROOT/target/windows-build.log" 2>&1
echo "cargo build exit=$?"

EXE="$ROOT/target/$TARGET/$PROFILE_DIR/$EXAMPLE.exe"
if [ ! -f "$EXE" ]; then
    echo "ERROR: build produced no exe at $EXE" >&2
    tail -40 "$ROOT/target/windows-build.log" >&2
    exit 1
fi

echo "── Packaging exe + Papirus assets ──"
rm -rf "$DIST"
mkdir -p "$DIST/assets/third_party"
cp "$EXE" "$DIST/boru.exe"
cp -r "$ROOT/assets/third_party/papirus" "$DIST/assets/third_party/papirus"

# Sanity: the bundle the loader probes must exist next to the exe.
PROBE="$DIST/assets/third_party/papirus/32/application-x-generic.svg"
if [ ! -f "$PROBE" ]; then
    echo "ERROR: packaged bundle missing probe file $PROBE" >&2
    exit 1
fi

ZIP="$ROOT/boru-windows-x86_64.zip"
rm -f "$ZIP"
( cd "$DIST" && zip -1 -r "$ZIP" boru.exe assets/ >/dev/null )

echo "── Verify ──"
file "$DIST/boru.exe"
stat -c 'exe size=%s bytes' "$DIST/boru.exe"
# sed (not head) so unzip never gets SIGPIPE from a closed pipe.
unzip -l "$ZIP" | sed -n '1,8p'
echo
echo "Package ready:"
echo "  exe: $DIST/boru.exe"
echo "  zip: $ZIP"
echo
echo "On the Windows host, extract the zip anywhere and run boru.exe — the"
echo "assets/ tree sits next to the exe so file-type icons resolve."
echo "Alternatively set BORU_PAPIRUS_ASSETS to the bundle root."
