#!/usr/bin/env bash
# ── CPU flamegraph for Boru GUI example ──────────────────────────
# Requires: cargo-flamegraph (`cargo install flamegraph`), perf on Linux
#
# Usage:
#   ./scripts/flamegraph.sh                            # default output
#   ./scripts/flamegraph.sh --features gui --output flame.svg
#   xvfb-run ./scripts/flamegraph.sh                   # headless server
#
# On headless machines without a display, run under xvfb-run:
#   sudo apt install xvfb
#   xvfb-run just perf-flamegraph
#
# The output SVG is written to ./flamegraph.svg by default.
# ======================================================================

set -euo pipefail

FEATURES="${FLAMEGRAPH_FEATURES:-gui}"
EXAMPLE="${FLAMEGRAPH_EXAMPLE:-iced_chat}"
OUTPUT="${1:-flamegraph.svg}"

# If arguments contain --output or --features, forward them
ARGS=("$@")

echo "→ Generating CPU flamegraph for $EXAMPLE (features=$FEATURES)"
echo "→ Output: $OUTPUT"
echo ""

# cargo flamegraph requires perf_event_paranoid ≤ 2 on Linux.
PARANOID=""
if [ -f /proc/sys/kernel/perf_event_paranoid ]; then
    PARANOID=$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo "3")
    if [ "$PARANOID" -gt 2 ] 2>/dev/null; then
        echo "⚠  perf_event_paranoid=$PARANOID (needs ≤ 2)."
        echo "   Run: echo 1 | sudo tee /proc/sys/kernel/perf_event_paranoid"
        echo "   Or the flamegraph will lack userspace symbols."
    fi
fi

exec cargo flamegraph \
    --features "$FEATURES" \
    --example "$EXAMPLE" \
    --output "$OUTPUT" \
    "${ARGS[@]}"
