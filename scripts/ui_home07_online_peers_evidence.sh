#!/usr/bin/env bash
# Evidence for UI-HOME-07 — refined Online Peers card (task t_85b7f19a).
#
# Captures the Home right rail in its two required states plus two
# interaction verifications, all from real friend + presence state:
#
#   empty            - fresh data dir with no friends -> intentional empty
#                      state ("No peers online") inside the min-height body,
#                      badge 0/0, "View all" preserved.
#   onepeer          - exactly one seeded friend marked online through the
#                      production friend-status path
#                      (boru_gui_set_peer_presence -> FriendEvent::StatusChanged,
#                      the same route real network events use). Badge 1/1,
#                      one 60 px two-line row: avatar + online dot + name +
#                      presence secondary line ("Online").
#   onepeer_viewall  - same one-peer state, then clicks the header "View all"
#                      action -> Screen::FriendRequests (interaction check).
#   onepeer_chat     - same one-peer state, then clicks the peer row
#                      -> Screen::Chat with the peer (OpenConversation
#                      preserved, interaction check).
#
# The card derives rows from live friend + presence state only — no sample
# data in the render path. The MCP presence action exists precisely so
# screenshot harnesses can capture truthful online states without a live peer
# connection (same pattern as the t_d4ca2ca4 evidence and UI-12 header
# evidence).
#
# Output: docs/ui-redesign/evidence/t_85b7f19a/
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/t_85b7f19a"
BINARY="$ROOT_DIR/target/debug/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
TASK_ID="t_85b7f19a"

mkdir -p "$OUTPUT_DIR"
[[ -x "$BINARY" ]] || { printf 'GUI binary not found: %s\n' "$BINARY" >&2; exit 1; }

find_display() {
    local display
    for display in $(seq 240 270); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]] && ! [[ -e "/tmp/.X${display}-lock" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
    printf 'no free X display in 240..270\n' >&2
    return 1
}

mcp() {
    local display=$1 port=$2 method=$3 params=$4
    DISPLAY=":$display" python3 "$MCP_CLIENT" "$port" "$method" "$params"
}

wait_main_window() {
    local display=$1
    local window_id=""
    local candidate name
    for _ in $(seq 1 60); do
        candidate=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | tail -n 1 || true)
        if [[ -n "$candidate" ]]; then
            name=$(DISPLAY=":$display" xdotool getwindowname "$candidate" 2>/dev/null || true)
            if [[ "$name" == *"v0."* ]]; then
                window_id="$candidate"
                break
            fi
        fi
        sleep 0.5
    done
    [[ -n "$window_id" ]] || { printf 'Boru main window never appeared\n' >&2; return 1; }
    printf '%s\n' "$window_id"
}

wait_mcp() {
    local display=$1 port=$2
    local attempt
    for attempt in $(seq 1 60); do
        if mcp "$display" "$port" boru_ping '{}' >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

# One deterministic friend (valid iroh PublicKey hex, "a1" * 32) in the same
# friends.json schema the app persists.
seed_one_friend() {
    local data_dir=$1
    python3 - "$data_dir" <<'PY'
import json, os, sys
data_dir = sys.argv[1]
pk = "a1" * 32
friends = {
    "friends": {
        pk: {
            "label": "Ada",
            "status": {"online": False, "last_offline_at_unix_ms": 1},
            "relationship": "friends",
        }
    }
}
with open(os.path.join(data_dir, "friends.json"), "w") as f:
    json.dump(friends, f, indent=2)
PY
}

launch_and_capture() {
    local state=$1 width=$2 height=$3
    local display data_dir mcp_port xvfb_pid app_pid window_id
    display=$(find_display)
    data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-home07.XXXXXX")
    mcp_port=$((18400 + display))
    xvfb_pid=""
    app_pid=""
    cleanup() {
        set +e
        [[ -n "${app_pid:-}" ]] && kill "$app_pid" 2>/dev/null
        [[ -n "${xvfb_pid:-}" ]] && kill "$xvfb_pid" 2>/dev/null
        wait "${app_pid:-}" 2>/dev/null
        wait "${xvfb_pid:-}" 2>/dev/null
        rm -rf "$data_dir"
    }
    trap cleanup RETURN

    if [[ "$state" != "empty" ]]; then
        seed_one_friend "$data_dir"
    fi

    Xvfb ":$display" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-home07-xvfb.log 2>&1 &
    xvfb_pid=$!
    sleep 0.5
    kill -0 "$xvfb_pid"

    DISPLAY=":$display" "$BINARY" \
        --data-dir "$data_dir" --no-dht --no-relay --name "Online Peers Evidence" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" \
        >/tmp/boru-home07-app-$display.log 2>&1 &
    app_pid=$!

    wait_mcp "$display" "$mcp_port" || { printf 'MCP never came up\n' >&2; exit 1; }
    sleep 2

    if [[ "$state" != "empty" ]]; then
        key=$(printf "a1%.0s" {1..32})
        for attempt in 1 2 3; do
            resp=$(mcp "$display" "$mcp_port" boru_gui_set_peer_presence \
                "{\"peer_id\":\"$key\",\"online\":true}" 2>&1 || true)
            if printf '%s' "$resp" | grep -q '"error"'; then
                sleep 0.2
            else
                break
            fi
        done
        printf 'presence a1 -> %s\n' "$(printf '%s' "$resp" | head -c 90)"
        sleep 2
    fi

    window_id=$(wait_main_window "$display") || exit 1
    DISPLAY=":$display" xdotool windowsize "$window_id" "$width" "$height"
    sleep 1
    DISPLAY=":$display" xdotool windowactivate --sync "$window_id" 2>/dev/null || true
    sleep 0.8

    local out="$OUTPUT_DIR/${TASK_ID}_${state}_${width}x${height}.png"
    DISPLAY=":$display" import -window "$window_id" "$out"
    printf 'captured %s\n' "$out"

    # ── Interaction verifications (only for the one-peer state) ─────────
    if [[ "$state" == "onepeer" ]]; then
        # 0) Hover treatment on the interactive peer row: park the cursor on
        #    the row (without clicking) and capture the hover surface.
        DISPLAY=":$display" xdotool mousemove 1100 190
        sleep 0.6
        local out_hover="$OUTPUT_DIR/${TASK_ID}_onepeer_hover_${width}x${height}.png"
        DISPLAY=":$display" import -window "$window_id" "$out_hover"
        printf 'captured %s\n' "$out_hover"

        # 1) View all -> Screen::FriendRequests. Coordinates verified from
        # the one-peer capture via tesseract TSV: "View all" sits at
        # (1176-1222, 138-148) in the Online Peers card header (the first
        # right-rail card). Click its centre.
        DISPLAY=":$display" xdotool mousemove 1200 143 click 1
        sleep 1.2
        local out_viewall="$OUTPUT_DIR/${TASK_ID}_onepeer_viewall_${width}x${height}.png"
        DISPLAY=":$display" import -window "$window_id" "$out_viewall"
        printf 'captured %s\n' "$out_viewall"

        # 2) Back to Home via the Friend Requests screen's "«Back" action
        #    (verified at 1184-1224, 51-61 in the view-all capture).
        DISPLAY=":$display" xdotool mousemove 1204 56 click 1
        sleep 1.2

        # 3) Peer row -> Screen::Chat. The single 60 px row spans
        #    y~160-220 (avatar at 986,164; name "Ada" at 1030,165; presence
        #    "Online" at 1030,188); click its middle (~x 1100, y 190).
        DISPLAY=":$display" xdotool mousemove 1100 190 click 1
        sleep 1.5
        local out_chat="$OUTPUT_DIR/${TASK_ID}_onepeer_chat_${width}x${height}.png"
        DISPLAY=":$display" import -window "$window_id" "$out_chat"
        printf 'captured %s\n' "$out_chat"
    fi
}

# ── States ────────────────────────────────────────────────────────────
launch_and_capture empty 1280 800
launch_and_capture onepeer 1280 800

printf 'Evidence written to %s\n' "$OUTPUT_DIR"
ls -la "$OUTPUT_DIR"
