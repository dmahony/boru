#!/usr/bin/env bash
# UI-13 deterministic Figure 4 visual regression.
#
# Injects the Figure 4 timeline into a temporary data directory, launches the
# real GUI under Xvfb, opens the seeded conversation through the test-action
# MCP endpoint, captures the 1280x800 window, and compares it to the committed
# baseline. On failure, the diff and JSON metrics remain in the evidence dir.
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BINARY=${BORU_BINARY:-$ROOT_DIR/target/debug/examples/boru}
MCP_CLIENT=$ROOT_DIR/scripts/ui_mcp.py
FIXTURE=$ROOT_DIR/scripts/figure4_fixture.py
COMPARE=$ROOT_DIR/scripts/compare_screenshot.py
EVIDENCE_DIR=$ROOT_DIR/docs/ui-redesign/evidence/ui-13-fixture
WIDTH=${BORU_WIDTH:-1280}
HEIGHT=${BORU_HEIGHT:-800}
BASELINE=$EVIDENCE_DIR/figure4-baseline-${WIDTH}x${HEIGHT}.png
ACTUAL=$EVIDENCE_DIR/figure4-current-${WIDTH}x${HEIGHT}.png
DIFF=$EVIDENCE_DIR/figure4-diff-${WIDTH}x${HEIGHT}.png
# The default 1280x800 run keeps the legacy metrics filename referenced by
# the committed README and baseline workflow; alternate sizes get a suffix.
if [[ "$WIDTH" == "1280" && "$HEIGHT" == "800" ]]; then
    METRICS=$EVIDENCE_DIR/figure4-comparison.json
else
    METRICS=$EVIDENCE_DIR/figure4-comparison-${WIDTH}x${HEIGHT}.json
fi

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
[[ -x "$BINARY" ]] || fail "GUI binary not found: $BINARY (build with: cargo build --features gui --example boru)"
[[ -x "$MCP_CLIENT" ]] || fail "MCP client is not executable: $MCP_CLIENT"
command -v Xvfb >/dev/null || fail "Xvfb is required for headless CI capture"
command -v xdotool >/dev/null || fail "xdotool is required for headless CI capture"
command -v import >/dev/null || fail "ImageMagick import is required for screenshot capture"
if [[ -f "$BASELINE" ]]; then
    HAS_BASELINE=1
else
    HAS_BASELINE=0
    echo "no baseline for ${WIDTH}x${HEIGHT}; capture-only mode"
fi

mkdir -p "$EVIDENCE_DIR"
display=""
data_dir=""
xvfb_pid=""
app_pid=""
cleanup() {
    set +e
    [[ -n "$app_pid" ]] && kill "$app_pid" 2>/dev/null || true
    [[ -n "$xvfb_pid" ]] && kill "$xvfb_pid" 2>/dev/null || true
    [[ -n "$app_pid" ]] && wait "$app_pid" 2>/dev/null || true
    [[ -n "$xvfb_pid" ]] && wait "$xvfb_pid" 2>/dev/null || true
    [[ -n "$data_dir" ]] && python3 "$FIXTURE" cleanup "$data_dir" >/dev/null 2>&1 || rm -rf "$data_dir"
}
trap cleanup EXIT

for candidate in $(seq 180 219); do
    if [[ ! -e "/tmp/.X11-unix/X${candidate}" && ! -e "/tmp/.X${candidate}-lock" ]]; then
        display=$candidate
        break
    fi
done
[[ -n "$display" ]] || fail "no free X display in 180..219"
data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-figure4-visual.XXXXXX")
# Pin the fixture's clock so the data (and date grouping) is reproducible for
# the duration of the run without depending on an operator's input.
python3 "$FIXTURE" inject "$data_dir" --now-ms "$(date +%s%3N)" >/dev/null

mcp_port=$((18600 + display))
Xvfb ":$display" -screen 0 "${WIDTH}x${HEIGHT}x24" -nolisten tcp >/tmp/boru-figure4-xvfb.log 2>&1 &
xvfb_pid=$!
sleep 0.5
kill -0 "$xvfb_pid" 2>/dev/null || fail "Xvfb failed to start"

DISPLAY=":$display" "$BINARY" \
    --data-dir "$data_dir" --no-dht --no-relay --name "UI-13 Visual Regression" \
    --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
    >/tmp/boru-figure4-app.log 2>&1 &
app_pid=$!

for attempt in $(seq 1 80); do
    if DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null 2>&1; then
        break
    fi
    sleep 0.25
done
DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null \
    || fail "GUI MCP endpoint did not become ready"

remote_pk=28d7ee8656$(printf 'ab%.0s' {1..27})
DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_gui_open_conversation \
    "{\"conversation_id\":\"$remote_pk\"}" >/dev/null

window_id=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' | head -n 1)
[[ -n "$window_id" ]] || fail "Boru window not found"
DISPLAY=":$display" xdotool windowsize "$window_id" "$WIDTH" "$HEIGHT"
sleep 0.8

# Wait for the timeline to settle. The deterministic history replay and the
# live system events that follow it ("Chat joined.", /help hint) arrive
# asynchronously, so a fixed sleep is racy: two runs could capture different
# states (earlier run only the replay, later run replay + live events).
# Poll until two consecutive frames are pixel-identical (or timeout).
settle_prev=""
settled=0
for settle_attempt in $(seq 1 30); do
    DISPLAY=":$display" import -window "$window_id" /tmp/boru-figure4-settle.png 2>/dev/null || true
    if [[ -n "$settle_prev" ]] && cmp -s /tmp/boru-figure4-settle.png "$settle_prev"; then
        settled=1
        break
    fi
    cp /tmp/boru-figure4-settle.png "$settle_prev" 2>/dev/null || true
    sleep 0.5
done
if [[ "$settled" == "1" ]]; then
    echo "timeline settled after ${settle_attempt} stability checks"
else
    echo "timeline did not fully settle within timeout; capturing anyway"
fi

DISPLAY=":$display" import -window "$window_id" "$ACTUAL"

if [[ "$HAS_BASELINE" == "1" ]]; then
    python3 "$COMPARE" "$ACTUAL" "$BASELINE" --diff "$DIFF" --metrics "$METRICS" \
        --tolerance "${BORU_PIXEL_TOLERANCE:-16}" \
        --max-mismatch "${BORU_MAX_MISMATCH:-0.005}"
    printf 'Visual regression artifacts:\n  actual:  %s\n  baseline: %s\n  diff:    %s\n  metrics: %s\n' "$ACTUAL" "$BASELINE" "$DIFF" "$METRICS"
else
    printf 'Capture artifact (no baseline to compare): %s\n' "$ACTUAL"
fi
