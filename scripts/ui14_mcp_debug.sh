#!/usr/bin/env bash
# Debug MCP navigation live: launch app under Xvfb, call navigate + snapshot, print raw responses.
set -u
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN="$ROOT/target/debug/examples/boru"
MCP="$ROOT/scripts/ui_mcp.py"
D=""
for c in $(seq 300 359); do
  if [[ ! -e "/tmp/.X11-unix/X$c" ]] && [[ ! -e "/tmp/.X$c-lock" ]]; then D=$c; break; fi
done
echo "display=$D"
DATA=$(mktemp -d /tmp/ui14test.XXXXXX)
Xvfb ":$D" -screen 0 1280x800x24 -nolisten tcp >/tmp/t-xvfb.log 2>&1 &
XV=$!
sleep 0.8
DISPLAY=":$D" "$BIN" --data-dir "$DATA" --no-dht --no-relay --name "T14" \
  --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$((18700 + D))" >/tmp/t-app.log 2>&1 &
APP=$!
PORT=$((18700 + D))
echo "port=$PORT"
for _ in $(seq 1 60); do
  if python3 "$MCP" "$PORT" boru_ping '{}' >/dev/null 2>&1; then break; fi
  sleep 0.25
done
echo "--- navigate file_sharing ---"
NAV=$(python3 "$MCP" "$PORT" boru_gui_navigate '{"destination":"file_sharing"}')
echo "$NAV"
AID=$(echo "$NAV" | python3 -c "import sys,json;print(json.load(sys.stdin)['result']['action_id'])" 2>/dev/null || true)
for i in 1 2 3 4 5 6 7 8 9 10; do
  sleep 0.6
  SC=$(python3 "$MCP" "$PORT" boru_get_gui_snapshot '{}' 2>/dev/null | python3 -c "import sys,json;d=json.load(sys.stdin);print(d['result']['active_screen'])" 2>/dev/null || echo ERR)
  echo "t=$((i*6))/10s screen=$SC"
done
WIN=$(DISPLAY=":$D" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
echo "win=$WIN"
if [[ -n "$WIN" ]]; then
  sleep 1
  DISPLAY=":$D" import -window "$WIN" /tmp/ui14-debug-shot.png 2>/dev/null
  echo "captured /tmp/ui14-debug-shot.png"
fi
echo "--- send_gui_action create_new_room ---"
python3 "$MCP" "$PORT" boru_send_gui_action '{"command":"create_new_room"}'
sleep 3
echo "--- snapshot 2 ---"
python3 "$MCP" "$PORT" boru_get_gui_snapshot '{}' | head -c 400
echo
kill "$APP" "$XV" 2>/dev/null
wait "$APP" 2>/dev/null
wait "$XV" 2>/dev/null
rm -rf "$DATA"
echo "DONE"
