#!/usr/bin/env bash
# UI-HOME-06 interaction checks: hover elevation + keyboard activation.
# 1. Hover: move the pointer over the first quick-action card and capture —
#    the hovered card should show the accent border + elevation shadow.
# 2. Keyboard: Ctrl+N must still open the create-room dialog (the global
#    shortcut path the home screen relies on for keyboard activation).
set -euo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=$ROOT/target/debug/examples/boru
TASK_ID="t_2577e385"
OUT="$ROOT/docs/ui-redesign/evidence/$TASK_ID"
mkdir -p "$OUT"

display=$(for d in $(seq 190 230); do [[ -e "/tmp/.X11-unix/X$d" ]] || [[ -e "/tmp/.X$d-lock" ]] || { echo "$d"; break; }; done)
data=$(mktemp -d "${TMPDIR:-/tmp}/boru-home06int.XXXXXX")
Xvfb ":$display" -screen 0 1600x900x24 -nolisten tcp >/tmp/boru-home06i-xvfb.log 2>&1 & xv=$!
sleep 0.5
DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "UI-HOME-06 interact" \
    >/tmp/boru-home06i-app.log 2>&1 & app=$!
win=''
for _ in $(seq 1 80); do
    win=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
    [[ -n "$win" ]] && break
    sleep 0.25
done
[[ -n "$win" ]] || { echo "no window"; kill "$app" "$xv" 2>/dev/null || exit 1; }
DISPLAY=":$display" xdotool windowsize "$win" 1600 900
sleep 5

# Hover over the first quick-action card (icon centre x~426, y~514; title area y~565).
DISPLAY=":$display" xdotool mousemove --sync 430 570
sleep 1
DISPLAY=":$display" import -window "$win" "$OUT/${TASK_ID}_hover_card1_1600x900.png"
# Move away so hover clears.
DISPLAY=":$display" xdotool mousemove --sync 50 500
sleep 0.5

# Keyboard: Ctrl+N opens the create-room dialog (global shortcut preserved).
DISPLAY=":$display" xdotool key --window "$win" ctrl+n
sleep 1.5
DISPLAY=":$display" import -window "$win" "$OUT/${TASK_ID}_keyboard_ctrln_1600x900.png"

kill "$app" "$xv" 2>/dev/null || true
wait "$app" 2>/dev/null || true
wait "$xv" 2>/dev/null || true
rm -rf "$data"
file "$OUT"/${TASK_ID}_hover_*_1600x900.png "$OUT"/${TASK_ID}_keyboard_*_1600x900.png
