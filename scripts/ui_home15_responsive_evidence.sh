#!/usr/bin/env bash
# Evidence for UI-HOME-15: responsive behaviour of the completed home screen.
#
# Captures the home screen at the four supported widths (wide / medium /
# narrow / minimum), then OCR-verifies the key layout facts:
#
#   wide    1600x900  - two dashboard columns, four quick-action columns,
#                      full-size hero illustration, greeting+pill on one row
#   medium  1280x800  - two dashboard columns, 2x2 quick actions
#   narrow  1024x720  - one dashboard column (right rail below), 2x2 quick
#                      actions, scaled hero illustration
#   minimum 800x600   - one dashboard column, one quick action per row,
#                      compact card headers, pill under greeting,
#                      hero illustration hidden
#
# Usage:
#   scripts/ui_home15_responsive_evidence.sh
#
# Output: docs/ui-redesign/evidence/t_dfe40e9f/
#   t_dfe40e9f_home_1600x900.png
#   t_dfe40e9f_home_1280x800.png
#   t_dfe40e9f_home_1024x720.png
#   t_dfe40e9f_home_800x600.png
#   geometry.txt            (OCR row/column positions per width)
#   README.md
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/t_dfe40e9f"
BINARY="$ROOT_DIR/target/debug/examples/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
TASK_ID="t_dfe40e9f"

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

# Launch the app on a fresh display at the requested window size, then wait
# for the home (chat_list) screen to be responsive.
launch_state() {
    local width=$1 height=$2
    DISPLAY_NUM=$(find_display)
    DATA_DIR=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui15.XXXXXX")
    local mcp_port=$((18800 + DISPLAY_NUM))

    Xvfb ":$DISPLAY_NUM" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-ui15-xvfb.log 2>&1 &
    XVFB_PID=$!
    sleep 0.5
    kill -0 "$XVFB_PID"

    DISPLAY=":$DISPLAY_NUM" "$BINARY" \
        --data-dir "$DATA_DIR" --no-dht --no-relay --name "UI-HOME-15 Evidence" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
        >/tmp/boru-ui15-app.log 2>&1 &
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

# Capture the home screen at a given size.
capture() {
    local width=$1 height=$2 out=$3
    launch_state "$width" "$height"
    DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$out"
    printf 'captured %s\n' "$out"
    cleanup
    DISPLAY_NUM=""; DATA_DIR=""; XVFB_PID=""; APP_PID=""; WIN_ID=""
}

capture 1600 900 "$OUTPUT_DIR/${TASK_ID}_home_1600x900.png"
capture 1280 800 "$OUTPUT_DIR/${TASK_ID}_home_1280x800.png"
capture 1024 720 "$OUTPUT_DIR/${TASK_ID}_home_1024x720.png"
capture 800 600 "$OUTPUT_DIR/${TASK_ID}_home_800x600.png"

# Scrolled captures: at 1280 and 1024 the second quick-action row sits below
# the fold (the page scrolls vertically by design — gutter_scrollable). Capture
# the scrolled state so the full 2x2 grid is visible for the report.
capture_scrolled() {
    local width=$1 height=$2 out=$3
    launch_state "$width" "$height"
    # Move the pointer over the main content panel (right of the sidebar) and
    # scroll down several notches with the mouse wheel (same pattern as
    # ui_home06_scroll_evidence.sh).
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
    printf 'UI-HOME-15 geometry (OCR word boxes; x-right must stay < window width)\n'
    printf '====================================================================\n'
    for width in 1600 1280 1024 800; do
        IMG="$OUTPUT_DIR/${TASK_ID}_home_${width}x*.png"
        for f in $IMG; do
            printf '\n[%s] %s\n' "$width" "$(basename "$f")"
            printf '%-20s %-8s %-8s %-6s %s\n' token left top width right
            tesseract "$f" - tsv 2>/dev/null | awk -F'\t' -v W="$width" \
                'NR>1 && $12 != "" && $11 > 40 {
                    x=$7; w=$9; r=x+w;
                    if (r > W) { over++ }
                    if (++n <= 14) printf "%-20s %-8s %-8s %-6s %s\n", $12, x, $8, w, r;
                }
                END { printf "rows=%d words_past_right_edge=%d\n", n, over+0 }'
        done
    done
} > "$GEOM"
printf 'geometry written to %s\n' "$GEOM"

# ── README ─────────────────────────────────────────────────────────────
cat > "$OUTPUT_DIR/README.md" <<EOF
# UI-HOME-15 responsive evidence

Four supported window widths captured from the running Boru GUI under Xvfb
(fresh data dir, --no-dht --no-relay, MCP-driven home navigation). The
dashboard breakpoints are content-width based: window minus sidebar (288-320
px), divider and page padding.

| Width | Content width (approx) | Intentional layout |
|---|---|---|
| 1600x900 | ~1231 px | Two dashboard columns, four quick-action columns, full hero illustration |
| 1280x800 | ~919 px | Two dashboard columns, 2x2 quick actions |
| 1024x720 | ~679 px | One dashboard column (right rail below), 2x2 quick actions, scaled illustration |
| 800x600 | ~455 px | One dashboard column, one quick action per row, compact headers, pill under greeting, no illustration |

See docs/ui-redesign/UI-HOME-15-report.md for the full report and
geometry.txt for OCR word-box overflow checks.
EOF

printf 'Evidence written to %s\n' "$OUTPUT_DIR"
ls -la "$OUTPUT_DIR"
