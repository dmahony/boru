#!/usr/bin/env bash
# UI-06 add-friend + friend-row context-menu functional checks via xdotool.
# Uses tesseract TSV to locate the sidebar elements, clicks them, and captures
# evidence. Requires a running app on $DISPLAY with seeded data; wraps its own
# launch to keep the harness self-contained.
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/ui-06-v4/functional"
BINARY="$ROOT_DIR/target/debug/examples/boru"
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

# Find the on-screen x,y centre of the first line matching the given regex in
# the given screenshot, using tesseract TSV (word-level boxes).  Only rows in
# the left sidebar (x < 280) are considered — every element we click here is a
# sidebar element.  Pass a minimum y (3rd arg) to skip higher sections.
find_text_xy() {
    local png=$1 pattern=$2 ymin=${3:-0}
    DISPLAY=":$display" tesseract "$png" stdout tsv 2>/dev/null |
        awk -F'\t' -v pat="$pattern" -v ymin="$ymin" 'NR>1 && $12 != "" && $7 < 280 && $8 >= ymin && $12 ~ pat { print int($7 + $9/2), int($8 + $10/2); exit }'
}

display=$(find_display)
data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui06-func2.XXXXXX")
mcp_port=$((18400 + display))
cleanup() {
    set +e
    [[ -n "${app_pid:-}" ]] && kill "$app_pid" 2>/dev/null
    [[ -n "${xvfb_pid:-}" ]] && kill "$xvfb_pid" 2>/dev/null
    wait 2>/dev/null
    rm -rf "$data_dir"
}
trap cleanup EXIT

python3 "$SEED_SCRIPT" "$data_dir" >/dev/null

Xvfb ":$display" -screen 0 "1280x800x24" -nolisten tcp >/tmp/boru-ui06-func2-xvfb.log 2>&1 &
xvfb_pid=$!
sleep 0.5

DISPLAY=":$display" "$BINARY" \
    --data-dir "$data_dir" --no-dht --no-relay --name "UI-06 Functional" \
    --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
    >/tmp/boru-ui06-func2-app.log 2>&1 &
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
capture "05_base_for_click"

# --- Add-friend field: click into it, type a public key, press Enter ---
# The field placeholder is "Add friend by key…".  Locate it inside the sidebar
# (x < 280) via the word "friend" from the placeholder; the field is centred in
# the sidebar, so click at (140, y).
read -r FX FY < <(find_text_xy "$OUTPUT_DIR/05_base_for_click.png" 'friend') || true
if [[ -n "${FX:-}" && -n "${FY:-}" && "$FX" -lt 280 ]]; then
    DISPLAY=":$display" xdotool mousemove --sync 140 "$FY"
    DISPLAY=":$display" xdotool click 1
    sleep 0.5
    # Type a valid 64-hex peer key (alice's key is a1*32) then Enter.
    DISPLAY=":$display" xdotool type --delay 20 "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1"
    DISPLAY=":$display" xdotool key Return
    sleep 1
    capture "06_add_friend_submitted"
    echo "add-friend: clicked (140,$FY), typed key, submitted"
else
    echo "add-friend: could not locate field; skipping"
fi

# --- Friend row context menu: click the overflow (⋮) button of a friend row.
# Use the FRIENDS-section long-name row (below the FRIENDS header ~y=475+).
read -r AX AY < <(find_text_xy "$OUTPUT_DIR/05_base_for_click.png" 'long-display-name' 400) || true
if [[ -n "${AX:-}" && -n "${AY:-}" && "$AY" -gt 400 ]]; then
    # Overflow button sits at the right edge of the row (~x=262 for 280px sidebar).
    DISPLAY=":$display" xdotool mousemove --sync 262 "$AY"
    DISPLAY=":$display" xdotool click 1
    sleep 1
    capture "07_friend_overflow_menu"
    echo "friend-overflow: clicked at (262,$AY)"
else
    echo "friend-overflow: could not locate FRIENDS-section Alice row; skipping"
fi

printf 'interaction pass complete\n'
