#!/usr/bin/env bash
# Evidence for the Home Online Peers card implemented with the reusable card
# shell (t_d4ca2ca4).
#
# Captures the Home screen right rail in its two required states:
#   empty     - fresh data dir with no friends -> CardShell "No peers online"
#               empty state, badge shows 0/0.
#   populated - seeds 8 friends, then drives real production presence state
#               via boru_gui_set_peer_presence (routes through the same
#               friend-status path real network events use) so the card
#               renders 8 online rows inside the bounded body: 5 rows visible
#               + vertical scrollbar, badge shows 8/8. The full row is the
#               preserved open-chat action (OpenConversation).
#
# The card derives rows from real friend + presence state only — no sample
# data in the render path. The MCP presence action exists precisely so
# screenshot harnesses can capture truthful online states without a live peer
# connection (same pattern as UI-12 header evidence).
#
# Output: docs/ui-redesign/evidence/ui-online-peers/
#   t_d4ca2ca4_empty_1280x800.png        - Home, empty Online Peers card
#   t_d4ca2ca4_populated_1280x800.png    - Home, 8 online peers, scrollable
#   t_d4ca2ca4_zoom_1280x800.png         - zoomed crop of the populated card
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/ui-online-peers"
BINARY="$ROOT_DIR/target/debug/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
TASK_ID="t_d4ca2ca4"

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

capture_state() {
    local state=$1 width=$2 height=$3
    local display data_dir mcp_port xvfb_pid app_pid window_id
    display=$(find_display)
    data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-online-peers.XXXXXX")
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

    if [[ "$state" == "populated" ]]; then
        python3 "$ROOT_DIR/scripts/seed_boru_data.py" "$data_dir" >/dev/null 2>&1 || true
        # Add five more deterministic friends (valid iroh PublicKeys) so the
        # card exceeds five rows.
        python3 - "$data_dir" <<'PY'
import json, os, sys
data_dir = sys.argv[1]
path = os.path.join(data_dir, "friends.json")
if os.path.exists(path):
    with open(path) as f:
        friends = json.load(f)
    peers = friends.setdefault("friends", {})
    for i, hexpair in enumerate(("77", "13", "4c", "0a", "11"), start=4):
        pk = hexpair * 32
        peers[pk] = {
            "label": f"Peer {i}",
            "status": {"online": False, "last_offline_at_unix_ms": 1},
            "relationship": "friends",
        }
    with open(path, "w") as f:
        json.dump(friends, f, indent=2)
PY
    fi

    Xvfb ":$display" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-online-peers-xvfb.log 2>&1 &
    xvfb_pid=$!
    sleep 0.5
    kill -0 "$xvfb_pid"

    DISPLAY=":$display" "$BINARY" \
        --data-dir "$data_dir" --no-dht --no-relay --name "Online Peers Evidence" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" \
        >/tmp/boru-online-peers-app-$display.log 2>&1 &
    app_pid=$!

    wait_mcp "$display" "$mcp_port" || { printf 'MCP never came up\n' >&2; exit 1; }
    sleep 2

    if [[ "$state" == "populated" ]]; then
        # Mark all 8 seeded friends online through the production path. The
        # GUI action queue rate-limits to 10 actions/sec, so pace the calls.
        for pk in "a1" "b2" "c3" "77" "13" "4c" "0a" "11"; do
            key=$(printf "$pk%.0s" {1..32})
            for attempt in 1 2 3; do
                resp=$(mcp "$display" "$mcp_port" boru_gui_set_peer_presence \
                    "{\"peer_id\":\"$key\",\"online\":true}" 2>&1 || true)
                if printf '%s' "$resp" | grep -q '"error"'; then
                    sleep 0.2
                else
                    break
                fi
            done
            printf 'presence %s -> %s\n' "$pk" "$(printf '%s' "$resp" | head -c 90)"
            sleep 0.15
        done
        sleep 3
    fi

    window_id=$(wait_main_window "$display") || exit 1
    DISPLAY=":$display" xdotool windowsize "$window_id" "$width" "$height"
    sleep 1
    DISPLAY=":$display" xdotool windowactivate --sync "$window_id" 2>/dev/null || true
    sleep 0.8

    local out="$OUTPUT_DIR/${TASK_ID}_${state}_${width}x${height}.png"
    DISPLAY=":$display" import -window "$window_id" "$out"
    printf 'captured %s\n' "$out"
}

# ── States ────────────────────────────────────────────────────────────
capture_state empty 1280 800
capture_state populated 1280 800

# Zoomed crop of the populated Online Peers card (right rail, below the
# hero/actions in the main panel). The rail starts around x=940 at 1280 wide.
convert "$OUTPUT_DIR/${TASK_ID}_populated_1280x800.png" \
    -crop 340x420+940+60 +repage \
    "$OUTPUT_DIR/${TASK_ID}_zoom_1280x800.png"
printf 'Evidence written to %s\n' "$OUTPUT_DIR"
ls -la "$OUTPUT_DIR"
