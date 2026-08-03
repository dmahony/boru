#!/usr/bin/env bash
# Evidence for the Home Tunnels card implemented with the reusable card shell
# (t_5f03f97d).
#
# Captures the Home screen with the Figure 3 rail Tunnels card in its truthful
# empty state ("No active tunnels"), the shell header (TUNNELS + count badge +
# "View all" header action wired to ShowCreateTunnelDialog), and a zoomed crop
# of the card itself. A live tunnel row requires a real friend and a created
# tunnel; the isolated run has neither, so the empty state is the honest
# evidence (matching the UI-10 rail evidence pattern — no sample data).
#
# Output: docs/ui-redesign/evidence/ui-tunnels-card/
#   t_5f03f97d_tunnels_1280x800.png  — Home at wide target size
#   t_5f03f97d_tunnels_600x720.png   — Home compact responsive
#   t_5f03f97d_tunnels_zoom_1280x800.png — zoomed crop of the Tunnels card
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/ui-tunnels-card"
BINARY="$ROOT_DIR/target/debug/examples/boru"
TASK_ID="t_5f03f97d"

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

display=$(find_display)
data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-tunnels-card.XXXXXX")
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
trap cleanup EXIT

Xvfb ":$display" -screen 0 1280x800x24 -nolisten tcp >/tmp/boru-tunnels-card-xvfb.log 2>&1 &
xvfb_pid=$!
sleep 0.5

DISPLAY=":$display" "$BINARY" --data-dir "$data_dir" --no-dht --no-relay \
    --name "Tunnels Card Evidence" >/tmp/boru-tunnels-card-app-$display.log 2>&1 &
app_pid=$!

# Wait for the MAIN window (skip the Tk splash, also titled "Boru").
window_id=""
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
[[ -n "$window_id" ]] || { printf 'Boru main window never appeared\n' >&2; exit 1; }

# Wait for the splash to close so it cannot steal focus.
for _ in $(seq 1 40); do
    count=$(DISPLAY=":$display" xdotool search --onlyvisible --name '^Boru' 2>/dev/null | wc -l)
    if [[ "$count" -le 1 ]]; then
        break
    fi
    sleep 0.5
done

sleep 1
DISPLAY=":$display" xdotool windowsize "$window_id" 1280 800
sleep 1
DISPLAY=":$display" xdotool windowfocus --sync "$window_id"
sleep 0.5

DISPLAY=":$display" import -window "$window_id" "$OUTPUT_DIR/${TASK_ID}_tunnels_1280x800.png"

# Zoomed crop of the Tunnels card in the right rail (main panel starts after
# the 280 px sidebar; the rail is the right-hand third of the main panel).
convert "$OUTPUT_DIR/${TASK_ID}_tunnels_1280x800.png" \
    -crop 400x320+860+380 +repage \
    "$OUTPUT_DIR/${TASK_ID}_tunnels_zoom_1280x800.png"

# Compact responsive capture. The rail reflows below the hero at narrow
# widths (window_width < 900). Use a taller window at the same 600 px width,
# then wheel-scroll the main panel down until the Tunnels card is visible.
DISPLAY=":$display" xdotool windowsize "$window_id" 600 1280
sleep 1.5
DISPLAY=":$display" xdotool mousemove 450 700
sleep 0.3
for _ in $(seq 1 40); do
    DISPLAY=":$display" xdotool click 5
    sleep 0.1
    DISPLAY=":$display" import -window "$window_id" /tmp/boru-tunnels-card-compact.png
    if tesseract /tmp/boru-tunnels-card-compact.png - 2>/dev/null | grep -q "No active tunnels"; then
        break
    fi
done
DISPLAY=":$display" import -window "$window_id" "$OUTPUT_DIR/${TASK_ID}_tunnels_600x1280.png"

printf 'Evidence written to %s\n' "$OUTPUT_DIR"
ls -la "$OUTPUT_DIR"
