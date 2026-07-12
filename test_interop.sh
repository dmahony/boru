#!/usr/bin/env bash
set -euo pipefail

# Workspace: /home/dan/boru-chat
# shellcheck disable=SC2046,SC2086,SC2124

cd /home/dan/boru-chat

# ── Config ─────────────────────────────────────────────────────────────
BUILD_MODE="${BUILD_MODE:-debug}"           # debug | release
INTEROP_BIN="target/${BUILD_MODE}/lan_test"
