#!/usr/bin/env bash
# BORU-HOME-12 capture: adapted from ui_home15_responsive_evidence.sh,
# pointed at the release binary and t_a38b6ffa evidence output.
# Captures the home screen at four widths (1600x900, 1280x800, 1024x720,
# 800x600) plus scrolled states, then writes an OCR geometry report.
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/t_a38b6ffa"
BINARY="$ROOT_DIR/target/release/examples/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
TASK_ID="t_a38b6ffa"

mkdir -p "$OUTPUT_DIR"
[[ -x "$BINARY" ]] || { printf 'GUI binary not found: %s\n' "$BINARY" >&2; exit 1; }
[[ -x "$MCP_CLIENT" ]] || { printf 'MCP helper not executable: %s\n' "$MCP_CLIENT" >&2; exit 1; }

DISPLAY_NUM=""
DATA_DIR=""
XVFB_PID=""
APP_PID=""
WIN_ID=""

cleanup() {
    set +e
    [[ -n "$APP_PID" ]] && kill "$APP_PID" 2>/dev/null
    [[ -n "$XVFB_PID" ]] && kill "$XVFB_PID" 2>/dev/null
    wait "$APP_PID" 2>/dev/null
    wait "$XVFB_PID" 2>/dev/null
    [[ -n "$DATA_DIR" ]] && rm -rf "$DATA_DIR"
}
trap cleanup EXIT

find_display() {
    local display
    for display in $(seq 250 270); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]] && ! [[ -e "/tmp/.X${display}-lock" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
    printf 'no free X display in 250..270\n' >&2
    return 1
}

mcp() {
    DISPLAY=":$DISPLAY_NUM" python3 "$MCP_CLIENT" "$1" "$2" "$3"
}

wait_main_window() {
    local window_id=""
    for _ in $(seq 1 60); do
        local candidate name
        candidate=$(DISPLAY=":$DISPLAY_NUM" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | tail -n 1 || true)
        if [[ -n "$candidate" ]]; then
            name=$(DISPLAY=":$DISPLAY_NUM" xdotool getwindowname "$candidate" 2>/dev/null || true)
            if [[ "$name" == *"v0."* ]]; then
                window_id="$candidate"
                break
            fi
        fi
        sleep 0.5
    done
    [[ -n "$window_id" ]] || { printf 'Boru main window never appeared\n' >&2; return 1; }
    for _ in $(seq 1 40); do
        local count
        count=$(DISPLAY=":$DISPLAY_NUM" xdotool search --onlyvisible --name '^Boru' 2>/dev/null | wc -l)
        if [[ "$count" -le 1 ]]; then
            break
        fi
        sleep 0.5
    done
    printf '%s\n' "$window_id"
}

launch_state() {
    local width=$1 height=$2
    DISPLAY_NUM=$(find_display)
    DATA_DIR=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui12.XXXXXX")
    local mcp_port=$((18800 + DISPLAY_NUM))

    Xvfb ":$DISPLAY_NUM" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-ui12-xvfb.log 2>&1 &
    XVFB_PID=$!
    sleep 0.5
    kill -0 "$XVFB_PID"

    DISPLAY=":$DISPLAY_NUM" "$BINARY" \
        --data-dir "$DATA_DIR" --no-dht --no-relay --name "UI-HOME-12 Evidence" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
        >/tmp/boru-ui12-app.log 2>&1 &
    APP_PID=$!

    local attempt
    for attempt in $(seq 1 60); do
        if DISPLAY=":$DISPLAY_NUM" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null 2>&1; then
            break
        fi
        sleep 0.25
    done
    DISPLAY=":$DISPLAY_NUM" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null
    sleep 2

    WIN_ID=$(wait_main_window)
    DISPLAY=":$DISPLAY_NUM" xdotool windowsize "$WIN_ID" "$width" "$height"
    sleep 1
    DISPLAY=":$DISPLAY_NUM" xdotool windowfocus --sync "$WIN_ID"
    sleep 0.5

    mcp "$mcp_port" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null
    sleep 1
}

capture() {
    local width=$1 height=$2 out=$3
    launch_state "$width" "$height"
    DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$out"
    printf 'captured %s\n' "$out"
    cleanup
    DISPLAY_NUM=""; DATA_DIR=""; XVFB_PID=""; APP_PID=""; WIN_ID=""
}

capture 1600 900 "$OUTPUT_DIR/${TASK_ID}_home_1600x900.png"
capture 1920 1080 "$OUTPUT_DIR/${TASK_ID}_home_1920x1080.png"
capture 1280 800 "$OUTPUT_DIR/${TASK_ID}_home_1280x800.png"
capture 1024 720 "$OUTPUT_DIR/${TASK_ID}_home_1024x720.png"
capture 800 600 "$OUTPUT_DIR/${TASK_ID}_home_800x600.png"

capture_scrolled() {
    local width=$1 height=$2 out=$3
    launch_state "$width" "$height"
    DISPLAY=":$DISPLAY_NUM" xdotool mousemove --sync $((width/2)) $((height/2))
    for _ in $(seq 1 10); do
        DISPLAY=":$DISPLAY_NUM" xdotool click 5
        sleep 0.2
    done
    sleep 1
    DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$out"
    printf 'captured %s\n' "$out"
    cleanup
    DISPLAY_NUM=""; DATA_DIR=""; XVFB_PID=""; APP_PID=""; WIN_ID=""
}

capture_scrolled 1280 800 "$OUTPUT_DIR/${TASK_ID}_home_1280x800_scrolled.png"
capture_scrolled 1024 720 "$OUTPUT_DIR/${TASK_ID}_home_1024x720_scrolled.png"
capture_scrolled 800 600 "$OUTPUT_DIR/${TASK_ID}_home_800x600_scrolled.png"

# ── OCR geometry report ────────────────────────────────────────────────
GEOM="$OUTPUT_DIR/geometry.txt"
{
    printf 'BORU-HOME-12 geometry (OCR word boxes; x-right must stay < window width)\n'
    printf '========================================================================\n'
    for width in 1600 1280 1024 800; do
        for f in "$OUTPUT_DIR/${TASK_ID}_home_${width}x"*.png; do
            [[ -f "$f" ]] || continue
            printf '\n[%s] %s\n' "$width" "$(basename "$f")"
            printf '%-22s %-7s %-7s %-6s %s\n' token left top width right
            tesseract "$f" - tsv 2>/dev/null | awk -F'\t' -v W="$width" \
                'NR>1 && $12 != "" && $11 > 40 {
                    x=$7; w=$9; r=x+w;
                    if (r > W) { over++ }
                    if (++n <= 14) printf "%-22s %-7s %-7s %-6s %s\n", $12, x, $8, w, r;
                }
                END { printf "rows=%d words_past_right_edge=%d\n", n, over+0 }'
        done
    done
} > "$GEOM"
printf 'geometry written to %s\n' "$GEOM"

printf 'Evidence written to %s\n' "$OUTPUT_DIR"
ls -la "$OUTPUT_DIR"
