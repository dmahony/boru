#!/usr/bin/env bash
# UI-15 evidence: final verification (t_77994a1a).
# Captures home screen at four required viewports for independent verification.
set -euo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=$ROOT/target/debug/boru
OUT=$ROOT/docs/ui-redesign/evidence/ui-15
mkdir -p "$OUT"

for spec in '1280 800' '1024 720' '1440 900' '1920 1080'; do
  read -r w h <<<"$spec"
  display=$((270 + w/100))
  data=$(mktemp -d /tmp/boru-ui15.XXXXXX)
  Xvfb ":$display" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/boru-ui15-xvfb-$w.log 2>&1 & xv=$!
  sleep 0.5
  DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "UI-15 $w" >/tmp/boru-ui15-app-$w.log 2>&1 & app=$!
  win=''
  for _ in $(seq 1 80); do
    win=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
    [[ -n "$win" ]] && break
    sleep 0.25
  done
  if [[ -z "$win" ]]; then
    kill "$app" "$xv" 2>/dev/null || true
    rm -rf "$data"
    echo "FAIL: window not found for ${w}x${h}" >&2
    exit 1
  fi
  DISPLAY=":$display" xdotool windowsize "$win" "$w" "$h"
  sleep 4
  DISPLAY=":$display" import -window "$win" "$OUT/t_77994a1a_home_${w}x${h}.png"
  kill "$app" "$xv" 2>/dev/null || true
  wait "$app" 2>/dev/null || true
  wait "$xv" 2>/dev/null || true
  rm -rf "$data"
  echo "OK: ${w}x${h}"
done
echo "ALL DONE"
file "$OUT"/t_77994a1a_home_*.png
