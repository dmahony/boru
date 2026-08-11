#!/usr/bin/env bash
# UI-HOME-06 keyboard activation check: Ctrl+N must still open the
# create-room dialog (global shortcut path). Under Xvfb the window needs
# explicit focus before synthetic keys register.
set -euo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=$ROOT/target/debug/boru
TASK_ID="t_2577e385"
OUT="$ROOT/docs/ui-redesign/evidence/$TASK_ID"
mkdir -p "$OUT"

display=$(for d in $(seq 190 230); do [[ -e "/tmp/.X11-unix/X$d" ]] || [[ -e "/tmp/.X$d-lock" ]] || { echo "$d"; break; }; done)
data=$(mktemp -d "${TMPDIR:-/tmp}/boru-home06kb.XXXXXX")
Xvfb ":$display" -screen 0 1600x900x24 -nolisten tcp >/tmp/boru-home06kb-xvfb.log 2>&1 & xv=$!
sleep 0.5
DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "UI-HOME-06 kb" \
    >/tmp/boru-home06kb-app.log 2>&1 & app=$!
win=''
for _ in $(seq 1 80); do
    win=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
    [[ -n "$win" ]] && break
    sleep 0.25
done
[[ -n "$win" ]] || { echo "no window"; kill "$app" "$xv" 2>/dev/null || exit 1; }
DISPLAY=":$display" xdotool windowsize "$win" 1600 900
sleep 5
# Focus the window, then send Ctrl+N.
DISPLAY=":$display" xdotool windowactivate --sync "$win" 2>/dev/null || DISPLAY=":$display" xdotool windowfocus "$win"
sleep 0.5
DISPLAY=":$display" xdotool key --clearmodifiers ctrl+n
sleep 2
DISPLAY=":$display" import -window "$win" "$OUT/${TASK_ID}_keyboard_ctrln_1600x900.png"
kill "$app" "$xv" 2>/dev/null || true
wait "$app" 2>/dev/null || true
wait "$xv" 2>/dev/null || true
rm -rf "$data"
file "$OUT"/${TASK_ID}_keyboard_ctrln_1600x900.png
