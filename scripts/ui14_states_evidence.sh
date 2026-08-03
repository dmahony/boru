#!/usr/bin/env bash
# UI-14 deterministic state-timeline capture (t_40d92fbe).
#
# Injects the UI-14 states spec (emoji-only, long unbroken, multiline, long
# wrapped, Queued/Sent/Delivered/Seen/Failed) into a temporary data dir,
# launches the real GUI under Xvfb, opens the seeded conversation through the
# test-action MCP endpoint, waits for the timeline to settle, and captures the
# window.  Same harness as ui13_visual_regression.sh but capture-only.
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BINARY=${BORU_BINARY:-$ROOT_DIR/target/debug/examples/boru}
MCP_CLIENT=$ROOT_DIR/scripts/ui_mcp.py
FIXTURE=$ROOT_DIR/scripts/figure4_fixture.py
SPEC=${BORU_SPEC:-$ROOT_DIR/docs/ui-redesign/evidence/ui-14/ui14-states-spec.json}
EVIDENCE_DIR=$ROOT_DIR/docs/ui-redesign/evidence/ui-14
WIDTH=${BORU_WIDTH:-1280}
HEIGHT=${BORU_HEIGHT:-800}
ACTUAL=$EVIDENCE_DIR/ui14_states_${WIDTH}x${HEIGHT}.png

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
[[ -x "$BINARY" ]] || fail "GUI binary not found: $BINARY (build with: cargo build --features gui --example boru)"
[[ -x "$MCP_CLIENT" ]] || fail "MCP client is not executable: $MCP_CLIENT"
[[ -f "$SPEC" ]] || fail "states spec not found: $SPEC"
command -v Xvfb >/dev/null || fail "Xvfb is required for headless capture"
command -v xdotool >/dev/null || fail "xdotool is required for headless capture"
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

for candidate in $(seq 220 259); do
    if [[ ! -e "/tmp/.X11-unix/X${candidate}" && ! -e "/tmp/.X${candidate}-lock" ]]; then
        display=$candidate
        break
    fi
done
[[ -n "$display" ]] || fail "no free X display in 220..259"

data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui14-states.XXXXXX")
python3 "$FIXTURE" inject "$data_dir" --spec "$SPEC" --now-ms "$(date +%s%3N)" >/dev/null

mcp_port=$((18800 + display))
Xvfb ":$display" -screen 0 "${WIDTH}x${HEIGHT}x24" -nolisten tcp >/tmp/boru-ui14-states-xvfb.log 2>&1 &
xvfb_pid=$!
sleep 0.5
kill -0 "$xvfb_pid" 2>/dev/null || fail "Xvfb failed to start"

DISPLAY=":$display" "$BINARY" \
    --data-dir "$data_dir" --no-dht --no-relay --name "UI-14 States" \
    --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
    >/tmp/boru-ui14-states-app.log 2>&1 &
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

settle_prev=""
settled=0
for settle_attempt in $(seq 1 30); do
    DISPLAY=":$display" import -window "$window_id" /tmp/boru-ui14-states-settle.png 2>/dev/null || true
    if [[ -n "$settle_prev" ]] && cmp -s /tmp/boru-ui14-states-settle.png "$settle_prev"; then
        settled=1
        break
    fi
    cp /tmp/boru-ui14-states-settle.png "$settle_prev" 2>/dev/null || true
    sleep 0.5
done
if [[ "$settled" == "1" ]]; then
    echo "timeline settled after ${settle_attempt} stability checks"
else
    echo "timeline did not fully settle within timeout; capturing anyway"
fi

DISPLAY=":$display" import -window "$window_id" "$ACTUAL"
echo "captured $ACTUAL"

# Also capture the top of the timeline: the states ladder (Read / Delivered /
# Sent) sits above the fold because the timeline is bottom-anchored. Scroll
# up with wheel-up over the chat column, settle, then capture a second frame.
chat_x=$((WIDTH - 250))
chat_y=$(((HEIGHT / 2) - 40))
DISPLAY=":$display" xdotool mousemove "$chat_x" "$chat_y"
for _ in $(seq 1 6); do
    DISPLAY=":$display" xdotool click 4
    sleep 0.15
done
sleep 1.0
TOP_ACTUAL=$EVIDENCE_DIR/ui14_states_top_${WIDTH}x${HEIGHT}.png
DISPLAY=":$display" import -window "$window_id" "$TOP_ACTUAL"
echo "captured $TOP_ACTUAL"
file "$ACTUAL" "$TOP_ACTUAL"
