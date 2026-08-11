#!/usr/bin/env bash
# Manual two-instance ready test: verify instances discover/connect, then capture.
set -euo pipefail
ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BINARY="$ROOT_DIR/target/debug/boru"
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/ui-08"
TOPIC="2222222222222222222222222222222222222222222222222222222222222222"
mkdir -p "$OUTPUT_DIR"

find_display() {
    local d
    for d in $(seq 170 190); do
        if ! [[ -e "/tmp/.X11-unix/X${d}" ]]; then printf '%s\n' "$d"; return 0; fi
    done
    return 1
}

DA=$(find_display)
DB=$(find_display)
DATA_A=$(mktemp -d /tmp/boru-ui08-rdy-a.XXXXXX)
DATA_B=$(mktemp -d /tmp/boru-ui08-rdy-b.XXXXXX)

cleanup() {
    set +e
    [[ -n "${PA:-}" ]] && kill "$PA" 2>/dev/null
    [[ -n "${PB:-}" ]] && kill "$PB" 2>/dev/null
    [[ -n "${XA:-}" ]] && kill "$XA" 2>/dev/null
    [[ -n "${XB:-}" ]] && kill "$XB" 2>/dev/null
    rm -rf "$DATA_A" "$DATA_B"
}
trap cleanup EXIT

Xvfb ":$DA" -screen 0 "1280x800x24" -nolisten tcp >/tmp/boru-ui08-rdy-xa.log 2>&1 & XA=$!
Xvfb ":$DB" -screen 0 "1280x800x24" -nolisten tcp >/tmp/boru-ui08-rdy-xb.log 2>&1 & XB=$!
sleep 0.6

DISPLAY=":$DA" "$BINARY" --data-dir "$DATA_A" --no-dht --no-relay --name "Ready A" \
    open "$TOPIC" >/tmp/boru-ui08-rdy-a.log 2>&1 & PA=$!
DISPLAY=":$DB" "$BINARY" --data-dir "$DATA_B" --no-dht --no-relay --name "Ready B" \
    open "$TOPIC" >/tmp/boru-ui08-rdy-b.log 2>&1 & PB=$!

for i in $(seq 1 30); do
    sleep 4
    # Look for neighbor/connection evidence in the app's own log (data dir).
    NEIGHBORS_A=$(grep -ci "neighbor\|connected to\|direct\|relay" "$DATA_A"/logs/*.log 2>/dev/null || true)
    echo "t=$((i*4))s neighbors-A-log-lines=$NEIGHBORS_A"
    if [[ "$NEIGHBORS_A" -gt 3 ]]; then
        echo "A appears connected at t=$((i*4))s"
        break
    fi
done

sleep 3
WID=$(DISPLAY=":$DA" xdotool search --sync --onlyvisible --name '^Boru' | head -n 1)
DISPLAY=":$DA" xdotool windowsize "$WID" 1280 800
sleep 0.5
DISPLAY=":$DA" import -window "$WID" "$OUTPUT_DIR/t_ed8af7fe_ready_1280x800.png"
echo "captured ready"
echo "=== A logs ==="
ls -la "$DATA_A/logs/" 2>/dev/null | head
find "$DATA_A" -name "*.log" -newer /tmp/boru-ui08-rdy-a.log 2>/dev/null | head
