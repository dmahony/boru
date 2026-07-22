#!/usr/bin/env bash
set -euo pipefail

# Workspace: /home/dan/iroh-gossip-chat
# shellcheck disable=SC2046,SC2086,SC2124

cd /home/dan/iroh-gossip-chat

# ── Config ─────────────────────────────────────────────────────────────
BUILD_MODE="${BUILD_MODE:-debug}"           # debug | release
INTEROP_BIN="target/${BUILD_MODE}/lan_test"
