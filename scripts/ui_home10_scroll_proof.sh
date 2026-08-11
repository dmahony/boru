#!/usr/bin/env bash
# UI-HOME-10 scroll proof: at a short window the home page must scroll
# vertically (content below the fold reachable), with no horizontal
# overflow. Captures the top and after scrolling to the bottom.
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/t_faa09772/scroll"
BINARY="$ROOT_DIR/target/debug/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
SEED_SCRIPT="$ROOT_DIR/scripts/seed_boru_data.py"
mkdir -p "$OUTPUT_DIR"

DISPLAY_NUM=""; DATA_DIR=""; XVFB_PID=""; APP_PID=""; WIN_ID=""
cleanup() {
    set +e
    [[ -n "$APP_PID" ]] && kill "$APP_PID" 2>/dev/null
    [[ -n "$XVFB_PID" ]] && kill "$XVFB_PID" 2>/dev/null
    wait "$APP_PID" 2>/dev/null; wait "$XVFB_PID" 2>/dev/null
    [[ -n "$DATA_DIR" ]] && rm -rf "$DATA_DIR"
}
trap cleanup EXIT

for display in $(seq 250 270); do
    if ! [[ -e "/tmp/.X11-unix/X${display}" ]]; then DISPLAY_NUM=$display; break; fi
done

DATA_DIR=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui10s.XXXXXX")
python3 "$SEED_SCRIPT" "$DATA_DIR" >/dev/null
PORT=$((18800 + DISPLAY_NUM))

Xvfb ":$DISPLAY_NUM" -screen 0 900x650x24 -nolisten tcp >/tmp/boru-ui10s-xvfb.log 2>&1 &
XVFB_PID=$!; sleep 0.5

DISPLAY=":$DISPLAY_NUM" "$BINARY" \
    --data-dir "$DATA_DIR" --no-dht --no-relay --name "UI-HOME-10 Scroll Proof" \
    --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$PORT" open \
    >/tmp/boru-ui10s-app.log 2>&1 &
APP_PID=$!
for _ in $(seq 1 60); do
    DISPLAY=":$DISPLAY_NUM" python3 "$MCP_CLIENT" "$PORT" boru_ping '{}' >/dev/null 2>&1 && break
    sleep 0.25
done
sleep 2
WIN_ID=$(DISPLAY=":$DISPLAY_NUM" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | tail -n 1)
DISPLAY=":$DISPLAY_NUM" xdotool windowsize "$WIN_ID" 900 650
sleep 1
DISPLAY=":$DISPLAY_NUM" python3 "$MCP_CLIENT" "$PORT" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null
sleep 1
LONG_PK=$(printf 'c3%.0s' {1..32})
DISPLAY=":$DISPLAY_NUM" python3 "$MCP_CLIENT" "$PORT" boru_gui_set_peer_presence "{\"peer_id\":\"$LONG_PK\",\"online\":true}" >/dev/null
sleep 1

echo "--- before scroll (main panel x>320) ---"
DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$OUTPUT_DIR/home_scroll_top_900x650.png"
tesseract "$OUTPUT_DIR/home_scroll_top_900x650.png" - tsv 2>/dev/null | awk -F'\t' '$12 != "" && $11 > 40 && $7 > 320 {print $8, $12}' | head -18

# Scroll: pointer over the main content, wheel down many times.
DISPLAY=":$DISPLAY_NUM" xdotool mousemove --sync 700 500
DISPLAY=":$DISPLAY_NUM" xdotool click --repeat 25 --delay 40 5
sleep 1

echo "--- after scroll (main panel x>320) ---"
DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$OUTPUT_DIR/home_scroll_bottom_900x650.png"
tesseract "$OUTPUT_DIR/home_scroll_bottom_900x650.png" - tsv 2>/dev/null | awk -F'\t' '$12 != "" && $11 > 40 && $7 > 320 {print $8, $12}' | head -18
ls -la "$OUTPUT_DIR"
