#!/usr/bin/env bash
# Evidence for UI-HOME-10: overflow / clipping / scroll audit.
#
# Captures the home screen in the two required states:
#
#   longname  - seeded fixture with a deliberately long display name
#               ("a-very-long-display-name-for-truncation-test-peer-42") and a
#               long local label passed via --name, so Online Peers rows, the
#               greeting and the sidebar identity all carry long text that
#               must WRAP (not clip) inside content-driven rows.
#   narrow    - a narrow window (1024x720, rail stacked) to prove the page
#               scrolls vertically and no horizontal overflow appears.
#
# Usage:
#   scripts/ui_home10_evidence.sh <label>   # label e.g. "after"
#
# Output: docs/ui-redesign/evidence/t_faa09772/<label>/
#   home_longname_1280x800_<label>.png
#   home_narrow_1024x720_<label>.png
#   geometry_<label>.txt   (OCR row y-positions: long row grows past 60 px)
set -euo pipefail

LABEL="${1:?usage: ui_home10_evidence.sh <label>}"

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/t_faa09772/$LABEL"
BINARY="$ROOT_DIR/target/debug/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
SEED_SCRIPT="$ROOT_DIR/scripts/seed_boru_data.py"
TASK_ID="t_faa09772"

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
    local state=$1 width=$2 height=$3
    DISPLAY_NUM=$(find_display)
    DATA_DIR=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui10.XXXXXX")
    local mcp_port=$((18800 + DISPLAY_NUM))

    if [[ "$state" == "longname" ]]; then
        python3 "$SEED_SCRIPT" "$DATA_DIR" >/dev/null
    fi

    Xvfb ":$DISPLAY_NUM" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-ui10-xvfb.log 2>&1 &
    XVFB_PID=$!
    sleep 0.5
    kill -0 "$XVFB_PID"

    # Long local label: exercises the greeting and sidebar identity wrap.
    local name_arg="UI-HOME-10 Evidence"
    if [[ "$state" == "longname" ]]; then
        name_arg="a-very-long-local-display-name-that-must-wrap-inside-the-sidebar-and-greeting-42"
    fi

    DISPLAY=":$DISPLAY_NUM" "$BINARY" \
        --data-dir "$DATA_DIR" --no-dht --no-relay --name "$name_arg" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
        >/tmp/boru-ui10-app.log 2>&1 &
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

    if [[ "$state" == "longname" ]]; then
        ALICE_PK=$(printf 'a1%.0s' {1..32})
        BOB_PK=$(printf 'b2%.0s' {1..32})
        LONG_PK=$(printf 'c3%.0s' {1..32})
        mcp "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$ALICE_PK\",\"online\":true}" >/dev/null
        sleep 0.5
        mcp "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$BOB_PK\",\"online\":false}" >/dev/null
        sleep 0.5
        mcp "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$LONG_PK\",\"online\":true}" >/dev/null
        sleep 1
    fi
}

# Capture the home screen at a given size.
# Usage: capture <state> <width> <height> <outfile>
capture() {
    local state=$1 width=$2 height=$3 out=$4
    launch_state "$state" "$width" "$height"
    DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$out"
    printf 'captured %s\n' "$out"
    cleanup
    DISPLAY_NUM=""; DATA_DIR=""; XVFB_PID=""; APP_PID=""; WIN_ID=""
}

capture longname 1280 800 "$OUTPUT_DIR/home_longname_1280x800_${LABEL}.png"
capture narrow 1024 720 "$OUTPUT_DIR/home_narrow_1024x720_${LABEL}.png"

# Geometry report: Online Peers long row height from the longname capture.
IMG="$OUTPUT_DIR/home_longname_1280x800_${LABEL}.png"
GEOM="$OUTPUT_DIR/geometry_${LABEL}.txt"
{
    printf 'geometry %s  (OCR word boxes; long display name must wrap, not clip)\n' "$LABEL"
    printf '%s\n' '----------------------------------------------'
    for header in 'ONLINE' 'RECENT' 'TUNNELS' 'MESH' 'a-very' 'Boru'; do
        read -r left top w h <<<"$(tesseract "$IMG" - tsv 2>/dev/null | awk -F'\t' -v re="$header" '$12 ~ re { print $7, $8, $9, $10; exit }')"
        printf '%-18s left=%-5s top=%-5s w=%-5s h=%s\n' "$header" "${left:-?}" "${top:-?}" "${w:-?}" "${h:-?}"
    done
} > "$GEOM"
printf 'geometry written to %s\n' "$GEOM"

printf 'Evidence written to %s\n' "$OUTPUT_DIR"
ls -la "$OUTPUT_DIR"
