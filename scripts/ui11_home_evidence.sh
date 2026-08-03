#!/usr/bin/env bash
set -euo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=$ROOT/target/debug/examples/boru
OUT=$ROOT/docs/ui-redesign/evidence/ui-11
mkdir -p "$OUT"
for spec in '1280 800' '1024 720' '1440 900' '1920 1080'; do
  read -r w h <<<"$spec"
  display=$((240 + w/100))
  data=$(mktemp -d /tmp/boru-ui11.XXXXXX)
  Xvfb ":$display" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/boru-ui11-xvfb.log 2>&1 & xv=$!
  sleep 0.5
  DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "UI-11 $w" >/tmp/boru-ui11-app-$w.log 2>&1 & app=$!
  win=''
  for _ in $(seq 1 80); do
    win=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
    [[ -n "$win" ]] && break
    sleep 0.25
  done
  if [[ -z "$win" ]]; then
    kill "$app" "$xv" 2>/dev/null || true
    rm -rf "$data"
    echo "window not found for ${w}x${h}" >&2
    exit 1
  fi
  DISPLAY=":$display" xdotool windowsize "$win" "$w" "$h"
  sleep 4
  DISPLAY=":$display" import -window "$win" "$OUT/t_24b1cb38_home_${w}x${h}.png"
  kill "$app" "$xv" 2>/dev/null || true
  wait "$app" 2>/dev/null || true
  wait "$xv" 2>/dev/null || true
  rm -rf "$data"
done
file "$OUT"/t_24b1cb38_home_*.png
