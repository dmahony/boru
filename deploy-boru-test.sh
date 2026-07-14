#!/usr/bin/env bash
#
# deploy-boru-test.sh
# Build the boru-chat iced_chat GUI for Linux (x86_64 + aarch64) and Windows,
# then deploy the binaries to each target machine's ~/boru-test/ directory.
#
# Targets:
#   localhost  — Linux x86_64 (this machine)
#   dragon     — Linux aarch64 (172.16.0.118)
#   windows    — Windows x86_64 (172.16.0.17)
#
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"
BORU_TEST="$HOME/boru-test"
BORU_TEST_DRAGON="dan@dragon:boru-test"
WINDOWS_HOST="dan@172.16.0.17"
WINDOWS_SSHPASS="slow123"

ARCH_X86_64="x86_64-unknown-linux-gnu"
ARCH_AARCH64="aarch64-unknown-linux-gnu"
ARCH_WIN="x86_64-pc-windows-gnu"

# ── Step 0: Install any missing targets ─────────────────────────────────
echo "=== Step 0: Ensure rustup targets ==="
rustup target add aarch64-unknown-linux-gnu 2>/dev/null || true

# ── Step 1: Build all three targets ─────────────────────────────────────
echo ""
echo "=== Step 1: Build ==="

echo "--- Building Linux x86_64 (local) ---"
cargo build --example iced_chat --features gui --release 2>&1

echo ""
echo "--- Building Windows x86_64 (cross) ---"
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
  cargo build --example iced_chat --features gui --release \
  --target "$ARCH_WIN" 2>&1

echo ""
echo "--- Building Linux aarch64 (cross for dragon) ---"
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  cargo build --example iced_chat --features gui --release \
  --target "$ARCH_AARCH64" 2>&1

# ── Step 2: Set up local ~/boru-test ────────────────────────────────────
echo ""
echo "=== Step 2: Local deployment ==="
mkdir -p "$BORU_TEST"
cp "target/release/examples/iced_chat" "$BORU_TEST/iced_chat-x86_64-linux"
ln -sf "iced_chat-x86_64-linux" "$BORU_TEST/iced_chat" 2>/dev/null || true
echo "  -> $BORU_TEST/iced_chat-x86_64-linux"

cp "target/$ARCH_AARCH64/release/examples/iced_chat" "$BORU_TEST/iced_chat-aarch64-linux"
echo "  -> $BORU_TEST/iced_chat-aarch64-linux"

cp "target/$ARCH_WIN/release/examples/iced_chat.exe" "$BORU_TEST/iced_chat-x86_64-windows.exe"
echo "  -> $BORU_TEST/iced_chat-x86_64-windows.exe"

# ── Step 3: Deploy to dragon (aarch64) ──────────────────────────────────
echo ""
echo "=== Step 3: Deploy to dragon ==="
ssh dan@dragon "mkdir -p ~/boru-test"
scp "target/$ARCH_AARCH64/release/examples/iced_chat" \
  "$BORU_TEST_DRAGON/iced_chat-aarch64-linux"
echo "  -> dragon:~/boru-test/iced_chat-aarch64-linux"

# ── Step 4: Deploy to Windows ───────────────────────────────────────────
echo ""
echo "=== Step 4: Deploy to Windows ==="
if command -v sshpass &>/dev/null; then
  sshpass -p "$WINDOWS_SSHPASS" ssh "$WINDOWS_HOST" "mkdir %USERPROFILE%\\boru-test" 2>/dev/null || true
  sshpass -p "$WINDOWS_SSHPASS" scp \
    "target/$ARCH_WIN/release/examples/iced_chat.exe" \
    "$WINDOWS_HOST:boru-test/iced_chat-x86_64-windows.exe"
  echo "  -> windows:~/boru-test/iced_chat-x86_64-windows.exe"
else
  echo "  WARNING: sshpass not installed — skipping Windows deploy."
  echo "  Install with: sudo apt-get install sshpass"
  echo "  Binary ready at: $BORU_TEST/iced_chat-x86_64-windows.exe"
fi

# ── Summary ─────────────────────────────────────────────────────────────
echo ""
echo "=== Done ==="
echo ""
echo "Local:   $BORU_TEST/"
ls -lh "$BORU_TEST/"
echo ""
echo "Dragon:  ssh dan@dragon 'ls -lh ~/boru-test/'"
echo "Windows: ssh dan@172.16.0.17 'dir %USERPROFILE%\\boru-test'"
