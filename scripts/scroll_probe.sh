#!/usr/bin/env bash
# Scroll-behavior probe for t_6f308ca5 (UI-13 scroll preservation).
# Starts Boru under Xvfb with seeded data + chat history, opens the Alice
# conversation, and exercises the five acceptance scroll scenarios:
#   1. after_open      — fresh conversation with history snaps to latest
#   2. scrolled_up     — wheel up shows older messages
#   3. live_append     — live incoming appends while scrolled up must NOT
#                        move the reading position
#   4. back_to_bottom  — wheel down returns to the latest
#   5. append_at_bottom— live incoming while at bottom snaps to the newest
# Each state is captured and OCR'd; the OCR text is written to a JSON report.
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BINARY="$ROOT_DIR/target/debug/examples/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
SEED_SCRIPT="$ROOT_DIR/scripts/seed_boru_data.py"
HISTORY_SCRIPT="$ROOT_DIR/scripts/seed_chat_history.py"
OUT="${1:-/tmp/scroll-probe}"
rm -rf "$OUT"; mkdir -p "$OUT"

find_display() {
    local display
    for display in $(seq 240 260); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]] && ! [[ -e "/tmp/.X${display}-lock" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
    printf 'no free display\n' >&2
    return 1
}

mcp() {
    DISPLAY=":$1" python3 "$MCP_CLIENT" "$2" "$3" "$4"
}

ALICE_PK=$(printf 'a1%.0s' {1..32})

capture() {  # display out
    local window_id
    window_id=$(DISPLAY=":$1" xdotool search --sync --onlyvisible --name '^Boru' | head -n 1)
    DISPLAY=":$1" xdotool windowsize "$window_id" 1280 800
    sleep 0.5
    DISPLAY=":$1" import -window "$window_id" "$2"
}

ocr_chat() {  # png  -> prints OCR of the chat message region
    local png=$1
    python3 - "$png" <<'PY'
import sys
from PIL import Image
im = Image.open(sys.argv[1]).convert("RGB")
w, h = im.size
# chat region: skip sidebar (left ~304px) and header (top ~60) and composer (bottom ~60)
crop = im.crop((320, 70, w - 24, h - 66))
crop = crop.resize((crop.width * 2, crop.height * 2), Image.LANCZOS)
crop.save("/tmp/ocr_crop.png")
PY
    tesseract /tmp/ocr_crop.png - 2>/dev/null | grep -v '^\s*$' || true
}

display=$(find_display)
data_dir=$(mktemp -d /tmp/boru-scroll.XXXXXX)
mcp_port=$((18900 + display))

python3 "$SEED_SCRIPT" "$data_dir" >/dev/null
python3 "$HISTORY_SCRIPT" "$data_dir" 60 >/dev/null

Xvfb ":$display" -screen 0 1280x800x24 -nolisten tcp >/tmp/boru-scroll-xvfb.log 2>&1 &
xvfb_pid=$!
sleep 0.5

DISPLAY=":$display" "$BINARY" \
    --data-dir "$data_dir" --no-dht --no-relay --name "Scroll Probe" \
    --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
    >/tmp/boru-scroll-app.log 2>&1 &
app_pid=$!

for attempt in $(seq 1 60); do
    if DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null 2>&1; then
        break
    fi
    sleep 0.25
done
DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null
sleep 2

mcp "$display" "$mcp_port" boru_gui_open_conversation "{\"conversation_id\":\"$ALICE_PK\"}" >/dev/null
sleep 2.5

echo "=== state 1: after open (expect latest: msg 58-60 visible) ==="
capture "$display" "$OUT/after_open.png"
ocr_chat "$OUT/after_open.png" > "$OUT/after_open.ocr"
sed -n '1,30p' "$OUT/after_open.ocr"

echo "=== state 2: scroll up 8 notches (expect older: msg ~10-20) ==="
mx=900; my=350
DISPLAY=":$display" xdotool mousemove --sync "$mx" "$my"
sleep 0.3
DISPLAY=":$display" xdotool click 1
sleep 0.4
DISPLAY=":$display" xdotool click --repeat 8 --delay 50 4
sleep 1.2
capture "$display" "$OUT/scrolled_up.png"
ocr_chat "$OUT/scrolled_up.png" > "$OUT/scrolled_up.ocr"
sed -n '1,30p' "$OUT/scrolled_up.ocr"

echo "=== state 3: live presence appends while scrolled up (position must stay) ==="
mcp "$display" "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$ALICE_PK\",\"online\":false}" >/dev/null
sleep 1.2
mcp "$display" "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$ALICE_PK\",\"online\":true}" >/dev/null
sleep 1.2
capture "$display" "$OUT/live_append_scrolled.png"
ocr_chat "$OUT/live_append_scrolled.png" > "$OUT/live_append_scrolled.ocr"
sed -n '1,30p' "$OUT/live_append_scrolled.ocr"

echo "=== state 4: wheel down to bottom (expect latest again) ==="
DISPLAY=":$display" xdotool mousemove --sync "$mx" "$my"
sleep 0.3
DISPLAY=":$display" xdotool click --repeat 60 --delay 30 5
sleep 1.5
capture "$display" "$OUT/back_to_bottom.png"
ocr_chat "$OUT/back_to_bottom.png" > "$OUT/back_to_bottom.ocr"
sed -n '1,30p' "$OUT/back_to_bottom.ocr"

echo "=== state 5: live appends while at bottom (expect newest msg visible) ==="
mcp "$display" "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$ALICE_PK\",\"online\":false}" >/dev/null
sleep 1.2
mcp "$display" "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$ALICE_PK\",\"online\":true}" >/dev/null
sleep 1.2
capture "$display" "$OUT/append_at_bottom.png"
ocr_chat "$OUT/append_at_bottom.png" > "$OUT/append_at_bottom.ocr"
sed -n '1,30p' "$OUT/append_at_bottom.ocr"

echo "captures: $(ls -la "$OUT"/*.png | wc -l)"
# Deterministic PASS/FAIL assertions over the OCR dumps (t_727c1d5e).
if python3 "$ROOT_DIR/scripts/scroll_probe_check.py" "$OUT"; then
    echo "scroll_probe: 5/5 PASS"
else
    echo "scroll_probe: FAIL (see state diagnostics above)"
    kill "$app_pid" 2>/dev/null || true
    kill "$xvfb_pid" 2>/dev/null || true
    rm -rf "$data_dir"
    exit 1
fi
kill "$app_pid" 2>/dev/null || true
kill "$xvfb_pid" 2>/dev/null || true
rm -rf "$data_dir"
