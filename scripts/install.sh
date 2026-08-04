#!/usr/bin/env bash
# ── Build and install Boru with backward-compatible symlinks ──────────
#
# Builds the project (library + examples), then creates a `boru-chat`
# symlink pointing to the `boru` example binary so users who
# invoke the old binary name still get a working application.
#
# Usage:
#   ./scripts/install.sh                       # debug build
#   ./scripts/install.sh --release             # release build
#   ./scripts/install.sh --release --features gui   # release with GUI
#   ./scripts/install.sh --release --features video-playback # GUI + inline video
#
# Inline video packaging and clean-machine validation are documented in
# docs/video-runtime-packaging.md. This script never modifies PATH or installs
# a developer GStreamer SDK implicitly.
set -euo pipefail

PROFILE="${1:-debug}"
FEATURES="${2:---features gui}"

# Map "debug" to the correct Cargo profile directory
if [ "$PROFILE" = "--release" ]; then
    CARGO_FLAGS="--release"
    TARGET_DIR="release"
else
    CARGO_FLAGS=""
    TARGET_DIR="debug"
fi

echo "→ Building boru-core (profile=$TARGET_DIR, features=$FEATURES)..."

# Build the library and the boru GUI example (the main application)
cargo build $CARGO_FLAGS $FEATURES --example boru

# Determine the example binary path
EXAMPLE_BIN="target/$TARGET_DIR/examples/boru"

if [ ! -f "$EXAMPLE_BIN" ]; then
    echo "error: expected example binary not found at $EXAMPLE_BIN" >&2
    exit 1
fi

# Create backward-compatible `boru-chat` symlink
if [ ! -L "target/$TARGET_DIR/boru-chat" ]; then
    ln -s "examples/boru" "target/$TARGET_DIR/boru-chat"
    echo "→ Created backward-compatible symlink: target/$TARGET_DIR/boru-chat → examples/boru"
else
    echo "→ Backward-compatible symlink already exists: target/$TARGET_DIR/boru-chat"
fi

echo ""
echo "✓ Build complete."
echo ""
echo "  Library:        target/$TARGET_DIR/libboru_core.rlib"
echo "  Application:    target/$TARGET_DIR/examples/boru"
echo "  Legacy alias:   target/$TARGET_DIR/boru-chat  (symlink, deprecated)"
echo ""
echo "  Run:  cargo run --example boru --features gui"
echo "  Or:   ./target/$TARGET_DIR/boru-chat"
echo ""
echo "  The 'boru-chat' name is deprecated — please migrate to 'boru-core'."
