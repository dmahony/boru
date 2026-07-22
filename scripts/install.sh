#!/usr/bin/env bash
# ── Build and install Boru with backward-compatible symlinks ──────────
#
# Builds the project (library + examples), then creates a `boru-chat`
# symlink pointing to the `iced_chat` example binary so users who
# invoke the old binary name still get a working application.
#
# Usage:
#   ./scripts/install.sh                       # debug build
#   ./scripts/install.sh --release             # release build
#   ./scripts/install.sh --release --features gui   # release with GUI
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

# Build the library and the iced_chat example (the main application)
cargo build $CARGO_FLAGS $FEATURES --example iced_chat

# Determine the example binary path
EXAMPLE_BIN="target/$TARGET_DIR/examples/iced_chat"

if [ ! -f "$EXAMPLE_BIN" ]; then
    echo "error: expected example binary not found at $EXAMPLE_BIN" >&2
    exit 1
fi

# Create backward-compatible `boru-chat` symlink
if [ ! -L "target/$TARGET_DIR/boru-chat" ]; then
    ln -s "examples/iced_chat" "target/$TARGET_DIR/boru-chat"
    echo "→ Created backward-compatible symlink: target/$TARGET_DIR/boru-chat → examples/iced_chat"
else
    echo "→ Backward-compatible symlink already exists: target/$TARGET_DIR/boru-chat"
fi

echo ""
echo "✓ Build complete."
echo ""
echo "  Library:        target/$TARGET_DIR/libboru_core.rlib"
echo "  Application:    target/$TARGET_DIR/examples/iced_chat"
echo "  Legacy alias:   target/$TARGET_DIR/boru-chat  (symlink, deprecated)"
echo ""
echo "  Run:  cargo run --example iced_chat --features gui"
echo "  Or:   ./target/$TARGET_DIR/boru-chat"
echo ""
echo "  The 'boru-chat' name is deprecated — please migrate to 'boru-core'."
