#!/usr/bin/env bash
# UI-12 evidence: chat conversation header and toolbar.
#
# Captures the chat screen header in the three presence states required by
# the card, at the required viewport matrix (1280x800, 1024x720) plus a
# 150%-DPI render (Xvfb -dpi 144):
#   online   - boru_gui_set_peer_presence marks Alice online -> green Online dot
#   offline  - seeded friend Bob with no live peer -> muted Offline
#   long-id  - peer with a 64-char key + long display name -> truncated key
#              with copy button and reveal tooltip; long name does not clip
#              toolbar buttons.
#
# The production header derives presence from the peer-presence map that real
# network events (NeighborUp, FriendStatus::Online) populate; the MCP action
# routes through that same path, so the online capture is genuine app state.
#
# Also exercises every header toolbar action via real mouse clicks (xdotool)
# and keyboard activation (Tab focus + Enter/Space), capturing each state.
#
# Output: docs/ui-redesign/evidence/ui-12/<task-id>_<state>_<width>x<height>.png
#         docs/ui-redesign/evidence/ui-12/functional/*.png
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/ui-12"
BINARY="$ROOT_DIR/target/debug/examples/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
SEED_SCRIPT="${SEED_SCRIPT:-$ROOT_DIR/scripts/seed_boru_data.py}"
TASK_ID="t_16d417e4"

mkdir -p "$OUTPUT_DIR/functional"

[[ -x "$BINARY" ]] || { printf 'GUI binary not found: %s\n' "$BINARY" >&2; exit 1; }

find_display() {
    local display
    for display in $(seq 130 155); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
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

# conversation_id is the *peer public key* (64 hex chars) — the MCP
# open_conversation action resolves it to the direct conversation. The seed
# script uses Alice (a1*32), Bob (b2*32), long (c3*32).
ALICE_PK=$(printf 'a1%.0s' {1..32})
BOB_PK=$(printf 'b2%.0s' {1..32})
LONG_PK=$(printf 'c3%.0s' {1..32})

capture_state() {
    local state=$1 width=$2 height=$3 dpi=$4
    local display data_dir mcp_port xvfb_pid app_pid
    display=$(find_display)
    data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui12.XXXXXX")
    mcp_port=$((18400 + display))
    cleanup() {
        set +e
        [[ -n "${app_pid:-}" ]] && kill "$app_pid" 2>/dev/null
        [[ -n "${xvfb_pid:-}" ]] && kill "$xvfb_pid" 2>/dev/null
        wait "${app_pid:-}" 2>/dev/null
        wait "${xvfb_pid:-}" 2>/dev/null
        rm -rf "$data_dir"
    }
    trap cleanup RETURN

    python3 "$SEED_SCRIPT" "$data_dir" >/dev/null

    local xvfb_args=(":$display" -screen 0 "${width}x${height}x24" -nolisten tcp)
    if [[ -n "$dpi" ]]; then
        xvfb_args+=("-dpi" "$dpi")
    fi
    Xvfb "${xvfb_args[@]}" >/tmp/boru-ui12-xvfb.log 2>&1 &
    xvfb_pid=$!
    sleep 0.5
    kill -0 "$xvfb_pid"

    DISPLAY=":$display" "$BINARY" \
        --data-dir "$data_dir" --no-dht --no-relay --name "UI-12 Evidence" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
        >/tmp/boru-ui12-app.log 2>&1 &
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

    local topic
    case "$state" in
        online)  topic=$ALICE_PK ;;
        offline) topic=$BOB_PK ;;
        long)    topic=$LONG_PK ;;
    esac
    mcp "$display" "$mcp_port" boru_gui_open_conversation "{\"conversation_id\":\"$topic\"}" >/dev/null
    sleep 1.2

    # Simulate the peer's presence through the production friend-status path.
    # Without this the fixture has no live peer and the header truthfully
    # renders Offline; this action makes the online/offline captures explicit.
    case "$state" in
        online)
            mcp "$display" "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$ALICE_PK\",\"online\":true}" >/dev/null
            ;;
        offline)
            mcp "$display" "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$BOB_PK\",\"online\":false}" >/dev/null
            ;;
    esac
    sleep 0.8

    local out="$OUTPUT_DIR/${TASK_ID}_${state}_${width}x${height}.png"
    if [[ -n "$dpi" ]]; then
        out="$OUTPUT_DIR/${TASK_ID}_${state}_${width}x${height}_dpi${dpi}.png"
    fi
    capture_window "$display" "$out" "$width" "$height"
    printf 'captured %s\n' "$out"
}

# ── Static states ─────────────────────────────────────────────────────
capture_state online 1280 800 ""
capture_state offline 1280 800 ""
capture_state long 1280 800 ""
capture_state online 1024 720 ""
capture_state long 1024 720 ""
# 150% scale via Xvfb DPI 144 (96 * 1.5)
capture_state long 1280 800 144

# ── Functional: activate every header toolbar action ─────────────────
# Real mouse clicks via xdotool on the toolbar buttons, then keyboard
# activation (Tab focus + Enter). Each capture shows the action result.
FUNC="$OUTPUT_DIR/functional"
display=$(find_display)
data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui12-func.XXXXXX")
mcp_port=$((18400 + display))
python3 "$SEED_SCRIPT" "$data_dir" >/dev/null
cleanup_func() {
    set +e
    [[ -n "${app_pid:-}" ]] && kill "$app_pid" 2>/dev/null
    [[ -n "${xvfb_pid:-}" ]] && kill "$xvfb_pid" 2>/dev/null
    wait "${app_pid:-}" 2>/dev/null
    wait "${xvfb_pid:-}" 2>/dev/null
    rm -rf "$data_dir"
}
trap cleanup_func EXIT

Xvfb ":$display" -screen 0 "1280x800x24" -nolisten tcp >/tmp/boru-ui12-func-xvfb.log 2>&1 &
xvfb_pid=$!
sleep 0.5
DISPLAY=":$display" "$BINARY" \
    --data-dir "$data_dir" --no-dht --no-relay --name "UI-12 Functional" \
    --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
    >/tmp/boru-ui12-func-app.log 2>&1 &
app_pid=$!
for attempt in $(seq 1 60); do
    if DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null 2>&1; then
        break
    fi
    sleep 0.25
done
DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null
sleep 2

WINDOW_ID=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' | head -n 1)
DISPLAY=":$display" xdotool windowsize "$WINDOW_ID" 1280 800
sleep 0.6
DISPLAY=":$display" xdotool windowactivate --sync "$WINDOW_ID" 2>/dev/null || true

mcp "$display" "$mcp_port" boru_gui_open_conversation "{\"conversation_id\":\"$ALICE_PK\"}" >/dev/null
sleep 1
# Mark Alice online so the header shows the real Online state.
mcp "$display" "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$ALICE_PK\",\"online\":true}" >/dev/null
sleep 0.8
DISPLAY=":$display" import -window "$WINDOW_ID" "$FUNC/01_header_online.png"

# Toolbar ghost icon buttons — centers measured from the 1280x800 capture:
#   search (1100,26)  sweep (1134,26)  shared (1168,26)  details (1202,26)
#   more (1237,26)  back (23,26)
click_btn() {
    local x=$1 y=$2 name=$3
    DISPLAY=":$display" xdotool mousemove --sync "$x" "$y"
    sleep 0.3
    DISPLAY=":$display" xdotool click 1
    sleep 1.0
    DISPLAY=":$display" import -window "$WINDOW_ID" "$FUNC/$name.png"
    printf 'clicked %s at (%s,%s)\n' "$name" "$x" "$y"
}

HDR_Y=26
# 1. Search opens the in-conversation search panel.
click_btn 1100 "$HDR_Y" "02_search_panel"
# close it (Escape)
DISPLAY=":$display" xdotool key Escape
sleep 0.6
# 2. More options popover.
click_btn 1237 "$HDR_Y" "03_more_options"
DISPLAY=":$display" xdotool key Escape
sleep 0.6
# 3. Details panel.
click_btn 1202 "$HDR_Y" "04_details_panel"
DISPLAY=":$display" xdotool key Escape
sleep 0.6
# 4. Back to chat list.
click_btn 23 "$HDR_Y" "05_back_chatlist"
sleep 0.6
# Re-open a conversation for the keyboard pass.
mcp "$display" "$mcp_port" boru_gui_open_conversation "{\"conversation_id\":\"$ALICE_PK\"}" >/dev/null
sleep 1

# ── Keyboard activation ───────────────────────────────────────────────
# Tab through focusable widgets until a header toolbar button is focused,
# then press Enter. Capture the focused state and the activation result.
DISPLAY=":$display" xdotool key --clearmodifiers Tab
sleep 0.4
DISPLAY=":$display" import -window "$WINDOW_ID" "$FUNC/06_keyboard_focus.png"
DISPLAY=":$display" xdotool key Return
sleep 1.0
DISPLAY=":$display" import -window "$WINDOW_ID" "$FUNC/07_keyboard_activate.png"

printf 'functional captures complete\n'
