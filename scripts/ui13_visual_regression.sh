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
BASELINE=$EVIDENCE_DIR/figure4-baseline-1280x800.png
ACTUAL=$EVIDENCE_DIR/figure4-current-1280x800.png
DIFF=$EVIDENCE_DIR/figure4-diff-1280x800.png
METRICS=$EVIDENCE_DIR/figure4-comparison.json
WIDTH=1280
HEIGHT=800

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
[[ -x "$BINARY" ]] || fail "GUI binary not found: $BINARY (build with: cargo build --features gui --example boru)"
[[ -x "$MCP_CLIENT" ]] || fail "MCP client is not executable: $MCP_CLIENT"
[[ -f "$BASELINE" ]] || fail "baseline not found: $BASELINE"
command -v Xvfb >/dev/null || fail "Xvfb is required for headless CI capture"
command -v xdotool >/dev/null || fail "xdotool is required for headless CI capture"
command -v import >/dev/null || fail "ImageMagick import is required for screenshot capture"

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
sleep 1  # Allow the deterministic history replay to settle before capture.

window_id=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' | head -n 1)
[[ -n "$window_id" ]] || fail "Boru window not found"
DISPLAY=":$display" xdotool windowsize "$window_id" "$WIDTH" "$HEIGHT"
sleep 0.8
DISPLAY=":$display" import -window "$window_id" "$ACTUAL"

python3 "$COMPARE" "$ACTUAL" "$BASELINE" --diff "$DIFF" --metrics "$METRICS" \
    --tolerance "${BORU_PIXEL_TOLERANCE:-16}" \
    --max-mismatch "${BORU_MAX_MISMATCH:-0.005}"
printf 'Visual regression artifacts:\n  actual:  %s\n  baseline: %s\n  diff:    %s\n  metrics: %s\n' "$ACTUAL" "$BASELINE" "$DIFF" "$METRICS"
