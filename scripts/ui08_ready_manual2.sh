#!/usr/bin/env bash
# Capture the ready state reliably: two instances, full-display capture.
set -euo pipefail
ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BINARY="$ROOT_DIR/target/debug/examples/boru"
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/ui-08"
TOPIC="3333333333333333333333333333333333333333333333333333333333333333"
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
if [[ "$DB" == "$DA" ]]; then
    for candidate in $(seq 170 190); do
        if [[ "$candidate" != "$DA" ]] && ! [[ -e "/tmp/.X${candidate}-lock" ]]; then
            DB="$candidate"
            break
        fi
    done
fi
[[ "$DA" != "$DB" ]] || { echo "could not find two distinct X displays" >&2; exit 1; }
DATA_A=$(mktemp -d /tmp/boru-ui08-rdy2-a.XXXXXX)
DATA_B=$(mktemp -d /tmp/boru-ui08-rdy2-b.XXXXXX)

cleanup() {
    set +e
    [[ -n "${PA:-}" ]] && kill "$PA" 2>/dev/null
    [[ -n "${PB:-}" ]] && kill "$PB" 2>/dev/null
    [[ -n "${XA:-}" ]] && kill "$XA" 2>/dev/null
    [[ -n "${XB:-}" ]] && kill "$XB" 2>/dev/null
    rm -rf "$DATA_A" "$DATA_B"
}
trap cleanup EXIT

Xvfb ":$DA" -screen 0 "1280x800x24" -nolisten tcp >/tmp/boru-ui08-rdy2-xa.log 2>&1 & XA=$!
Xvfb ":$DB" -screen 0 "1280x800x24" -nolisten tcp >/tmp/boru-ui08-rdy2-xb.log 2>&1 & XB=$!
sleep 0.6

DISPLAY=":$DA" "$BINARY" --data-dir "$DATA_A" --no-dht --no-relay --name "Ready A" \
    >/tmp/boru-ui08-rdy2-a.log 2>&1 & PA=$!
DISPLAY=":$DB" "$BINARY" --data-dir "$DATA_B" --no-dht --no-relay --name "Ready B" \
    >/tmp/boru-ui08-rdy2-b.log 2>&1 & PB=$!

sleep 20

# Wait for a visible Boru window on display A
WID=""
for i in $(seq 1 20); do
    WID=$(DISPLAY=":$DA" xdotool search --onlyvisible --name 'Boru' 2>/dev/null | head -n 1 || true)
    [[ -n "$WID" ]] && break
    sleep 1
done
echo "window id A: '$WID'"
if [[ -n "$WID" ]]; then
    DISPLAY=":$DA" xdotool windowsize "$WID" 1280 800 || true
    sleep 1
    DISPLAY=":$DA" xdotool windowactivate --sync "$WID" 2>/dev/null || true
    sleep 1
fi
sleep 2
# Full-display capture is robust even if window mapping is flaky.
DISPLAY=":$DA" import -window root "$OUTPUT_DIR/t_ed8af7fe_ready_1280x800.png"
echo "captured ready (root)"
