#!/usr/bin/env bash
# Evidence for the Recent Activity card (t_8a9f9181).
#
# Captures the Figure 3 right-rail Recent Activity card in its two required
# states at the wide target window (1280x800):
#   empty     - fresh data dir with no activity events -> truthful
#               "No recent activity" empty state via CardShell
#   populated - seeded fixture + real friend-status events routed through the
#               production handle_friend_event -> push_activity path, so the
#               rows are genuine app state, not invented entries. The seeded
#               long-named peer demonstrates the ellipsis truncation.
#
# Relative timestamps are rendered by the shared presentation utility and are
# recomputed on each 1 Hz ConnMonitorTick re-render, satisfying the "timestamps
# update at a reasonable interval" acceptance item.
#
# Output: docs/ui-redesign/evidence/ui-activity/
#   t_8a9f9181_activity_empty_1280x800.png
#   t_8a9f9181_activity_populated_1280x800.png
#   t_8a9f9181_activity_populated_zoom_1280x800.png  (cropped rail)
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/ui-activity"
BINARY="$ROOT_DIR/target/debug/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
SEED_SCRIPT="$ROOT_DIR/scripts/seed_boru_data.py"
TASK_ID="t_8a9f9181"

mkdir -p "$OUTPUT_DIR"
[[ -x "$BINARY" ]] || { printf 'GUI binary not found: %s\n' "$BINARY" >&2; exit 1; }
[[ -x "$MCP_CLIENT" ]] || { printf 'MCP helper not executable: %s\n' "$MCP_CLIENT" >&2; exit 1; }

find_display() {
    local display
    for display in $(seq 220 240); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
    printf 'no free X display in 220..240\n' >&2
    return 1
}

mcp() {
    DISPLAY=":$1" python3 "$MCP_CLIENT" "$2" "$3" "$4"
}

capture_window() {
    local display=$1 output=$2 width=$3 height=$4
    local window_id
    window_id=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' | head -n 1)
    DISPLAY=":$display" xdotool windowsize "$window_id" "$width" "$height"
    sleep 0.6
    DISPLAY=":$display" import -window "$window_id" "$output"
}

capture_state() {
    local state=$1
    local display data_dir mcp_port xvfb_pid app_pid
    display=$(find_display)
    data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui-activity.XXXXXX")
    mcp_port=$((18600 + display))
    cleanup() {
        set +e
        [[ -n "${app_pid:-}" ]] && kill "$app_pid" 2>/dev/null
        [[ -n "${xvfb_pid:-}" ]] && kill "$xvfb_pid" 2>/dev/null
        wait "${app_pid:-}" 2>/dev/null
        wait "${xvfb_pid:-}" 2>/dev/null
        rm -rf "$data_dir"
    }
    trap cleanup RETURN

    if [[ "$state" == "populated" ]]; then
        python3 "$SEED_SCRIPT" "$data_dir" >/dev/null
    fi

    Xvfb ":$display" -screen 0 "1280x800x24" -nolisten tcp >/tmp/boru-ui-activity-xvfb.log 2>&1 &
    xvfb_pid=$!
    sleep 0.5
    kill -0 "$xvfb_pid"

    DISPLAY=":$display" "$BINARY" \
        --data-dir "$data_dir" --no-dht --no-relay --name "UI Activity Evidence" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
        >/tmp/boru-ui-activity-app.log 2>&1 &
    app_pid=$!

    local attempt
    for attempt in $(seq 1 60); do
        if DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null 2>&1; then
            break
        fi
        sleep 0.25
    done
    DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null
    sleep 2

    # Home (chat list) shows the Figure 3 rail with the Recent Activity card.
    mcp "$display" "$mcp_port" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null
    sleep 1

    if [[ "$state" == "populated" ]]; then
        # Route real friend-status events through the production
        # handle_friend_event path (has_been_seen=true for seeded friends),
        # which pushes genuine activity events into recent_activity.
        # Alice online, Bob offline, then the long-named peer online so the
        # row demonstrates the 64-char ellipsis truncation.
        ALICE_PK=$(printf 'a1%.0s' {1..32})
        BOB_PK=$(printf 'b2%.0s' {1..32})
        LONG_PK=$(printf 'c3%.0s' {1..32})
        mcp "$display" "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$ALICE_PK\",\"online\":true}" >/dev/null
        sleep 0.5
        mcp "$display" "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$BOB_PK\",\"online\":false}" >/dev/null
        sleep 0.5
        mcp "$display" "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$LONG_PK\",\"online\":true}" >/dev/null
        sleep 1
    fi

    local out="$OUTPUT_DIR/${TASK_ID}_activity_${state}_1280x800.png"
    capture_window "$display" "$out" 1280 800
    printf 'captured %s\n' "$out"
}

capture_state empty
capture_state populated

# Zoomed crop of the Recent Activity card in the populated capture. The card
# sits in the right rail below Online Peers; crop the rail region.
POPULATED="$OUTPUT_DIR/${TASK_ID}_activity_populated_1280x800.png"
convert "$POPULATED" \
    -crop 500x420+760+180 +repage \
    "$OUTPUT_DIR/${TASK_ID}_activity_populated_zoom_1280x800.png"

printf 'Evidence written to %s\n' "$OUTPUT_DIR"
ls -la "$OUTPUT_DIR"
