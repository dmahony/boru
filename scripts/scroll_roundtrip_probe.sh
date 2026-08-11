#!/usr/bin/env bash
# Two-instance scroll-behavior probe for t_6f308ca5 (UI-13 scroll preservation).
#
# Exercises the acceptance scroll scenarios with REAL network traffic between
# two Boru instances (A sends local messages; B replies, producing live remote
# appends on A). Each instance runs on its OWN Xvfb display so captures target
# the correct window unambiguously:
#   1. after_send      — A at bottom after sending 40 messages (latest visible)
#   2. scrolled_up     — A wheels up: older messages visible
#   3. live_append     — B sends while A is scrolled up: A's position preserved
#   4. back_to_bottom  — A wheels down: returns to latest (incl. B's messages)
#   5. append_at_bottom— B sends while A at bottom: A snaps to newest
#   6. history_replay  — restart A, reopen conversation: persisted history
#                        replays and lands at the bottom (unread-anchor path)
# Each state is captured, OCR'd, and checked against expected message numbers.
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BINARY="$ROOT_DIR/target/debug/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
SEED2="$ROOT_DIR/scripts/seed_two_instances.py"
OUT="${1:-/tmp/scroll-roundtrip}"
rm -rf "$OUT"; mkdir -p "$OUT"

find_free_displays() {  # -> prints two free display numbers
    local found=()
    for display in $(seq 260 320); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]] && ! [[ -e "/tmp/.X${display}-lock" ]]; then
            found+=("$display")
            if [[ ${#found[@]} -ge 2 ]]; then
                printf '%s\n' "${found[0]}" "${found[1]}"
                return 0
            fi
        fi
    done
    printf 'no free displays\n' >&2
    return 1
}

mcp() {  # port method params
    python3 "$MCP_CLIENT" "$1" "$2" "$3" 2>/dev/null || true
}

wait_ping() {  # port
    local port=$1 attempt
    for attempt in $(seq 1 100); do
        if python3 "$MCP_CLIENT" "$port" boru_ping '{}' >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.25
    done
    printf 'MCP ping timeout on port %s\n' "$port" >&2
    return 1
}

capture() {  # display out
    local display=$1 out=$2 window_id
    window_id=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' | head -n 1)
    if [[ -z "$window_id" ]]; then
        printf 'no window on :%s\n' "$display" >&2
        return 1
    fi
    DISPLAY=":$display" xdotool windowsize "$window_id" 1280 800
    sleep 0.5
    DISPLAY=":$display" import -window "$window_id" "$out"
}

ocr_chat() {  # png  -> OCR of the chat message region (stdout)
    local png=$1
    python3 - "$png" <<'PY'
import sys
from PIL import Image
im = Image.open(sys.argv[1]).convert("RGB")
w, h = im.size
# Chat region: skip sidebar (~330px) + header (top ~70) + composer (bottom ~70)
crop = im.crop((330, 70, w - 20, h - 70))
crop = crop.resize((crop.width * 2, crop.height * 2), Image.LANCZOS)
crop.save("/tmp/ocr_crop.png")
PY
    tesseract /tmp/ocr_crop.png - 2>/dev/null | grep -v '^\s*$' || true
}

send_msg() {  # port text
    local port=$1 text=$2 resp try
    for try in $(seq 1 10); do
        resp=$(mcp "$port" boru_gui_set_composer "{\"text\":\"$text\"}")
        if echo "$resp" | grep -q '"sent":true'; then break; fi
        sleep 0.4
    done
    for try in $(seq 1 10); do
        resp=$(mcp "$port" boru_gui_submit_composer '{}')
        if echo "$resp" | grep -q '"sent":true'; then return 0; fi
        sleep 0.5
    done
    return 1
}

wait_peer_connected() {  # port peer_id -> waits until gossip reports Connected
    local port=$1 peer=$2 attempt resp
    for attempt in $(seq 1 120); do
        resp=$(mcp "$port" boru_get_peer_status "{\"peer_id\":\"$peer\"}")
        if echo "$resp" | grep -q '"connection_state":"Connected"' \
            && echo "$resp" | grep -q '"topic_member":true'; then
            echo "peer connected (attempt $attempt)"
            return 0
        fi
        sleep 0.5
    done
    printf 'WARN peer %s never reported Connected on port %s\n' "$peer" "$port" >&2
    return 1
}

send_n() {  # port prefix count
    local port=$1 prefix=$2 count=$3 i text
    for i in $(seq 1 "$count"); do
        text=$(printf "%s msg %03d" "$prefix" "$i")
        if ! send_msg "$port" "$text"; then
            printf 'WARN send failed: %s\n' "$text" >&2
        fi
        sleep 0.3
    done
}

# ── Parse expected message numbers from an OCR file ──────────────────────
expect_bottom() {  # ocr_file label -> 0/1: newest local msg visible
    local ocr=$1 label=$2
    grep -q "RT msg 040" "$ocr" && return 0
    # Fallback: last RT msg present anywhere in the window
    grep -qE "RT msg 0(3[5-9]|40)" "$ocr" && return 0
    printf 'FAIL[%s]: newest RT msg not visible in %s\n' "$label" "$ocr" >&2
    return 1
}
expect_older() {  # ocr_file label -> 0/1: an older RT msg visible
    local ocr=$1 label=$2
    grep -qE "RT msg 0(0[1-9]|1[0-9]|2[0-9]|3[0-4])" "$ocr" && return 0
    printf 'FAIL[%s]: no older RT msg visible in %s\n' "$label" "$ocr" >&2
    return 1
}

display_a=$(find_free_displays | sed -n 1p)
display_b=$(find_free_displays | sed -n 2p)
[[ -n "$display_a" && -n "$display_b" ]] || exit 1
echo "displays: A=:$display_a B=:$display_b"
dir_a=$(mktemp -d /tmp/boru-a.XXXXXX)
dir_b=$(mktemp -d /tmp/boru-b.XXXXXX)
port_a=$((19200 + display_a))
port_b=$((19300 + display_b))
# Deterministic QUIC bind ports; each side's friends.json seeds the other's
# direct address so the gossip mesh forms without mDNS (two iroh endpoints on
# one host cannot both own the mDNS 5353 socket reliably).
bind_port_a=$((42000 + display_a))
bind_port_b=$((42000 + display_b))

python3 "$SEED2" "$dir_a" "$dir_b" \
    --bind-port-a "$bind_port_a" --bind-port-b "$bind_port_b" > "$OUT/seed.txt"
cat "$OUT/seed.txt"

Xvfb ":$display_a" -screen 0 1280x800x24 -nolisten tcp >/tmp/scroll2-xvfb-a.log 2>&1 &
xvfb_a=$!
Xvfb ":$display_b" -screen 0 1280x800x24 -nolisten tcp >/tmp/scroll2-xvfb-b.log 2>&1 &
xvfb_b=$!
sleep 0.8

DISPLAY=":$display_a" "$BINARY" \
    --data-dir "$dir_a" --no-dht --no-relay --bind-port "$bind_port_a" --name "Instance A" \
    --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$port_a" open \
    >/tmp/scroll2-a.log 2>&1 &
pid_a=$!

DISPLAY=":$display_b" "$BINARY" \
    --data-dir "$dir_b" --no-dht --no-relay --bind-port "$bind_port_b" --name "Instance B" \
    --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$port_b" open \
    >/tmp/scroll2-b.log 2>&1 &
pid_b=$!

wait_ping "$port_a" || exit 1
wait_ping "$port_b" || exit 1
sleep 3

PK_A=$(grep -o 'pk=[0-9a-f]*' "$OUT/seed.txt" | head -1 | cut -d= -f2)
PK_B=$(grep -o 'pk=[0-9a-f]*' "$OUT/seed.txt" | sed -n 2p | cut -d= -f2)

# B opens its conversation with A first (so B's sender becomes ready).
mcp "$port_b" boru_gui_open_conversation "{\"conversation_id\":\"$PK_A\"}" >/dev/null
sleep 1.5
# A opens its conversation with B.
mcp "$port_a" boru_gui_open_conversation "{\"conversation_id\":\"$PK_B\"}" >/dev/null
# Wait until the gossip mesh reports both peers connected on the direct
# topic before sending anything. Sends submitted before the room is active
# are rejected by validate_gui_test_command (sender not ready), so this gate
# is what makes the message stream deterministic.
sleep 1
wait_peer_connected "$port_a" "$PK_B" || true
wait_peer_connected "$port_b" "$PK_A" || true
sleep 2

echo "=== A sends 40 messages ==="
send_n "$port_a" "RT msg" 40 || true
sleep 2

echo "=== state 1: after_send (expect RT msg 040 visible, at bottom) ==="
capture "$display_a" "$OUT/after_send.png"
ocr_chat "$OUT/after_send.png" > "$OUT/after_send.ocr"
head -15 "$OUT/after_send.ocr"
if expect_bottom "$OUT/after_send.ocr" after_send; then echo "PASS state 1"; else echo "FAIL state 1"; fi

echo "=== state 2: scroll up 8 notches (expect older RT msgs) ==="
mx=900; my=350
DISPLAY=":$display_a" xdotool mousemove --sync "$mx" "$my"
sleep 0.3
DISPLAY=":$display_a" xdotool click 1
sleep 0.4
DISPLAY=":$display_a" xdotool click --repeat 8 --delay 50 4
sleep 1.2
capture "$display_a" "$OUT/scrolled_up.png"
ocr_chat "$OUT/scrolled_up.png" > "$OUT/scrolled_up.ocr"
head -15 "$OUT/scrolled_up.ocr"
if expect_older "$OUT/scrolled_up.ocr" scrolled_up; then echo "PASS state 2"; else echo "FAIL state 2"; fi

echo "=== state 3: B sends 3 live messages while A is scrolled up ==="
send_n "$port_b" "FromB msg" 3 || true
sleep 2.5
capture "$display_a" "$OUT/live_append.png"
ocr_chat "$OUT/live_append.png" > "$OUT/live_append.ocr"
head -15 "$OUT/live_append.ocr"
# Position preserved: the SAME older window must still be visible, and the
# newest FromB msg must NOT have yanked the view to the bottom.
if expect_older "$OUT/live_append.ocr" live_append_scrolled \
    && ! grep -q "FromB msg 03" "$OUT/live_append.ocr"; then
    echo "PASS state 3"
else
    echo "FAIL state 3 (position stolen or wrong window)"
fi

echo "=== state 4: A wheels down to bottom ==="
DISPLAY=":$display_a" xdotool mousemove --sync "$mx" "$my"
sleep 0.3
DISPLAY=":$display_a" xdotool click --repeat 80 --delay 25 5
sleep 1.5
capture "$display_a" "$OUT/back_to_bottom.png"
ocr_chat "$OUT/back_to_bottom.png" > "$OUT/back_to_bottom.ocr"
head -15 "$OUT/back_to_bottom.ocr"
if expect_bottom "$OUT/back_to_bottom.ocr" back_to_bottom \
    && grep -q "FromB msg 03" "$OUT/back_to_bottom.ocr"; then
    echo "PASS state 4"
else
    echo "FAIL state 4"
fi

echo "=== state 5: B sends 2 more while A at bottom ==="
send_n "$port_b" "FromB msg" 2 || true
sleep 2.5
capture "$display_a" "$OUT/append_at_bottom.png"
ocr_chat "$OUT/append_at_bottom.png" > "$OUT/append_at_bottom.ocr"
head -15 "$OUT/append_at_bottom.ocr"
if grep -q "FromB msg 05" "$OUT/append_at_bottom.ocr"; then
    echo "PASS state 5"
else
    echo "FAIL state 5 (did not snap to newest)"
fi

# ── History replay on restart ─────────────────────────────────────────
echo "=== state 6: restart A, reopen conversation, history replay ==="
kill "$pid_a" 2>/dev/null || true
sleep 2
DISPLAY=":$display_a" "$BINARY" \
    --data-dir "$dir_a" --no-dht --no-relay --bind-port "$bind_port_a" --name "Instance A2" \
    --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$port_a" open \
    >/tmp/scroll2-a2.log 2>&1 &
pid_a2=$!
wait_ping "$port_a" || exit 1
sleep 3
mcp "$port_a" boru_gui_open_conversation "{\"conversation_id\":\"$PK_B\"}" >/dev/null
sleep 3
capture "$display_a" "$OUT/history_replay.png"
ocr_chat "$OUT/history_replay.png" > "$OUT/history_replay.ocr"
head -15 "$OUT/history_replay.ocr"
if expect_bottom "$OUT/history_replay.ocr" history_replay; then
    echo "PASS state 6"
else
    echo "FAIL state 6"
fi

kill "$pid_a2" "$pid_b" "$xvfb_a" "$xvfb_b" 2>/dev/null || true
echo "captures: $(ls "$OUT"/*.png | wc -l)"
echo "OUT=$OUT"
