#!/usr/bin/env bash
# Evidence for UI-HOME-09: spacing / hierarchy / vertical rhythm standardisation.
#
# Captures the full home screen (Figure 3 dashboard) in the two required
# widths (wide 1600x900 and medium 1280x800) plus a populated variant at the
# reference width, so before/after spacing can be compared:
#
#   empty      - fresh data dir with no activity/tunnels -> truthful empty
#                rail states (Online Peers / Recent Activity / Tunnels)
#   populated  - seeded fixture + real friend-status events routed through the
#                production handle_friend_event -> push_activity path, so the
#                rows are genuine app state, not invented entries
#
# Usage:
#   scripts/ui_home09_evidence.sh <label>   # label e.g. "before" / "after"
#
# Output: docs/ui-redesign/evidence/t_a24fbc67/<label>/
#   home_1600x900_<label>.png
#   home_1280x800_<label>.png
#   home_populated_1280x800_<label>.png
#   geometry_<label>.txt   (card header y-positions + gap measurements)
set -euo pipefail

LABEL="${1:?usage: ui_home09_evidence.sh <label>}"

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/t_a24fbc67/$LABEL"
BINARY="$ROOT_DIR/target/debug/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
SEED_SCRIPT="$ROOT_DIR/scripts/seed_boru_data.py"
TASK_ID="t_a24fbc67"

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
    DATA_DIR=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui09.XXXXXX")
    local mcp_port=$((18800 + DISPLAY_NUM))

    if [[ "$state" == "populated" ]]; then
        python3 "$SEED_SCRIPT" "$DATA_DIR" >/dev/null
    fi

    Xvfb ":$DISPLAY_NUM" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-ui09-xvfb.log 2>&1 &
    XVFB_PID=$!
    sleep 0.5
    kill -0 "$XVFB_PID"

    DISPLAY=":$DISPLAY_NUM" "$BINARY" \
        --data-dir "$DATA_DIR" --no-dht --no-relay --name "UI-HOME-09 Evidence" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
        >/tmp/boru-ui09-app.log 2>&1 &
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

    if [[ "$state" == "populated" ]]; then
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

capture empty 1600 900 "$OUTPUT_DIR/home_1600x900_${LABEL}.png"
capture empty 1280 800 "$OUTPUT_DIR/home_1280x800_${LABEL}.png"
capture populated 1280 800 "$OUTPUT_DIR/home_populated_1280x800_${LABEL}.png"

# Geometry report: card header y-positions from the medium empty capture.
IMG="$OUTPUT_DIR/home_1280x800_${LABEL}.png"
GEOM="$OUTPUT_DIR/geometry_${LABEL}.txt"
{
    printf 'geometry %s  (y = first text row top for each card header)\n' "$LABEL"
    printf '%s\n' '----------------------------------------------'
    for header in 'HELLO\|GOOD' 'ONLINE' 'RECENT' 'TUNNELS' 'MESH'; do
        read -r left top w h <<<"$(tesseract "$IMG" - tsv 2>/dev/null | awk -F'\t' -v re="$header" '$12 ~ re { print $7, $8, $9, $10; exit }')"
        printf '%-18s left=%-5s top=%-5s w=%-5s h=%s\n' "$header" "${left:-?}" "${top:-?}" "${w:-?}" "${h:-?}"
    done
} > "$GEOM"
printf 'geometry written to %s\n' "$GEOM"

printf 'Evidence written to %s\n' "$OUTPUT_DIR"
ls -la "$OUTPUT_DIR"
