#!/usr/bin/env bash
set -euo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=$ROOT/target/debug/boru
OUT=$ROOT/docs/ui-redesign/evidence/ui-11
mkdir -p "$OUT"
data=$(mktemp -d /tmp/boru-qa2.XXXXXX)
Xvfb ":212" -screen 0 1280x800x24 -nolisten tcp >/tmp/boru-qa2-xvfb.log 2>&1 & xv=$!
sleep 0.5
DISPLAY=":212" "$BIN" --data-dir "$data" --no-dht --no-relay --name "UI-11 QA2" >/tmp/boru-qa2-app.log 2>&1 & app=$!
win=''
for _ in $(seq 1 80); do
  win=$(DISPLAY=":212" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
  [[ -n "$win" ]] && break
  sleep 0.25
done
[[ -n "$win" ]]
DISPLAY=":212" xdotool windowsize "$win" 1280 800
sleep 2
# Click the Create Public Room card (first of the four in the action row).
DISPLAY=":212" xdotool mousemove --sync 400 710 click 1
sleep 1.5
DISPLAY=":212" import -window "$win" "$OUT/t_d9f6a827_public_dialog_1280x800.png"
echo "captured public dialog"
DISPLAY=":212" xdotool key Escape
sleep 0.5
# Click the Share Files card (fourth) — should open the file picker.
DISPLAY=":212" xdotool mousemove --sync 860 710 click 1
sleep 2
DISPLAY=":212" import -window "$win" "$OUT/t_d9f6a827_files_flow_1280x800.png"
echo "captured files flow"
kill -0 "$app" && echo "app alive after both clicks"
kill "$app" "$xv" 2>/dev/null || true
wait "$app" 2>/dev/null || true
wait "$xv" 2>/dev/null || true
rm -rf "$data"
