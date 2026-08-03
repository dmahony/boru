#!/usr/bin/env bash
set -euo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=$ROOT/target/debug/examples/boru
OUT=$ROOT/docs/ui-redesign/evidence/ui-11
mkdir -p "$OUT"
data=$(mktemp -d /tmp/boru-qa.XXXXXX)
Xvfb ":211" -screen 0 1280x800x24 -nolisten tcp >/tmp/boru-qa-xvfb.log 2>&1 & xv=$!
sleep 0.5
DISPLAY=":211" "$BIN" --data-dir "$data" --no-dht --no-relay --name "UI-11 QA" >/tmp/boru-qa-app.log 2>&1 & app=$!
win=''
for _ in $(seq 1 80); do
  win=$(DISPLAY=":211" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
  [[ -n "$win" ]] && break
  sleep 0.25
done
[[ -n "$win" ]]
DISPLAY=":211" xdotool windowsize "$win" 1280 800
sleep 2
DISPLAY=":211" import -window "$win" "$OUT/t_d9f6a827_home_1280x800.png"
# Click each quick-action card coordinate and confirm the app stays alive.
for spec in 'public 400' 'group 550' 'friend 700' 'files 860'; do
  read -r name x <<<"$spec"
  DISPLAY=":211" xdotool mousemove --sync "$x" 710 click 1
  sleep 1
  kill -0 "$app"
  echo "$name: click handled; process remained alive"
  DISPLAY=":211" xdotool key Escape || true
  sleep 0.5
done
kill "$app" "$xv" 2>/dev/null || true
wait "$app" 2>/dev/null || true
wait "$xv" 2>/dev/null || true
rm -rf "$data"
file "$OUT"/t_d9f6a827_home_1280x800.png
