#!/usr/bin/env bash
# UI-06 REQUESTS-section evidence: scroll the sidebar down and capture the
# REQUESTS section (Manage Requests secondary button + empty-state row) for
# both empty and populated data.  Also captures the 1024x720 empty sidebar.
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/ui-06-v4/functional"
BINARY="$ROOT_DIR/target/debug/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
SEED_SCRIPT="${SEED_SCRIPT:-$ROOT_DIR/scripts/seed_boru_data.py}"

mkdir -p "$OUTPUT_DIR"

find_display() {
    local d
    for d in $(seq 99 119); do
        if ! [[ -e "/tmp/.X11-unix/X${d}" ]]; then printf '%s\n' "$d"; return 0; fi
    done
    return 1
}

capture() {
    local name=$1
    local window_id
    window_id=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' | head -n 1)
    DISPLAY=":$display" import -window "$window_id" "$OUTPUT_DIR/$name.png"
    printf 'captured %s\n' "$OUTPUT_DIR/$name.png"
}

launch() {
    local seed_dir=$1 width=$2 height=$3
    display=$(find_display)
    data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui06-req.XXXXXX")
    mcp_port=$((18600 + display))
    if [[ -n "$seed_dir" ]]; then cp -a "$seed_dir/." "$data_dir/"; fi
    Xvfb ":$display" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-ui06-req-xvfb.log 2>&1 &
    xvfb_pid=$!
    sleep 0.5
    DISPLAY=":$display" "$BINARY" \
        --data-dir "$data_dir" --no-dht --no-relay --name "UI-06 Evidence" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
        >/tmp/boru-ui06-req-app.log 2>&1 &
    app_pid=$!
    for i in $(seq 1 60); do
        DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null 2>&1 && break
        sleep 0.25
    done
    DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null
    sleep 2
    WINDOW_ID=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' | head -n 1)
    DISPLAY=":$display" xdotool windowactivate --sync "$WINDOW_ID" 2>/dev/null || true
    DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null
    sleep 1
}

cleanup_all() {
    set +e
    [[ -n "${app_pid:-}" ]] && kill "$app_pid" 2>/dev/null
    [[ -n "${xvfb_pid:-}" ]] && kill "$xvfb_pid" 2>/dev/null
    wait 2>/dev/null
    rm -rf "${data_dir:-}"
}
trap cleanup_all EXIT

# Empty state, 1024x720: sidebar may show REQUESTS after a scroll.
launch "" 1024 720
DISPLAY=":$display" xdotool mousemove --sync 140 400
DISPLAY=":$display" xdotool click 5  # scroll down
sleep 0.5
DISPLAY=":$display" xdotool click 5
sleep 0.5
DISPLAY=":$display" xdotool click 5
sleep 0.5
DISPLAY=":$display" xdotool click 5
sleep 1
capture "08_empty_1024_requests_scrolled"
DISPLAY=":$display" xdotool click 4  # scroll back up
sleep 0.5
DISPLAY=":$display" xdotool click 4
sleep 0.5
DISPLAY=":$display" xdotool click 4
sleep 0.5
DISPLAY=":$display" xdotool click 4
sleep 1
capture "09_empty_1024_top"
kill "$app_pid" "$xvfb_pid" 2>/dev/null
wait 2>/dev/null
unset app_pid xvfb_pid
rm -rf "$data_dir"
unset data_dir

# Populated state, 1280x800: REQUESTS section shows the pending request.
SEED_DIR=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui06-req-seed.XXXXXX")
python3 "$SEED_SCRIPT" "$SEED_DIR" >/dev/null
launch "$SEED_DIR" 1280 800
DISPLAY=":$display" xdotool mousemove --sync 140 400
for _ in $(seq 1 6); do DISPLAY=":$display" xdotool click 5; sleep 0.4; done
sleep 1
capture "10_populated_1280_requests_scrolled"
kill "$app_pid" "$xvfb_pid" 2>/dev/null
wait 2>/dev/null
rm -rf "$data_dir" "$SEED_DIR"

printf 'requests evidence pass complete\n'
