#!/usr/bin/env bash
set -euo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=$ROOT/target/debug/examples/boru
for action in public group friend files; do
  display=$((250 + ${#action}))
  data=$(mktemp -d /tmp/boru-ui11-action.XXXXXX)
  Xvfb ":$display" -screen 0 1280x800x24 -nolisten tcp >/tmp/boru-ui11-action-xvfb.log 2>&1 & xv=$!
  sleep 0.5
  DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "UI-11 $action" >/tmp/boru-ui11-action-$action.log 2>&1 & app=$!
  win=''
  for _ in $(seq 1 80); do
    win=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
    [[ -n "$win" ]] && break
    sleep 0.25
  done
  [[ -n "$win" ]]
  DISPLAY=":$display" xdotool windowsize "$win" 1280 800
  sleep 1
  case "$action" in
    public) x=400 ;;
    group) x=550 ;;
    friend) x=700 ;;
    files) x=860 ;;
  esac
  DISPLAY=":$display" xdotool mousemove --sync "$x" 710 click 1
  sleep 1
  DISPLAY=":$display" xdotool key Escape || true
  sleep 1
  kill -0 "$app"
  echo "$action: click handled; process remained alive"
  kill "$app" "$xv" 2>/dev/null || true
  wait "$app" 2>/dev/null || true
  wait "$xv" 2>/dev/null || true
  rm -rf "$data"
done
