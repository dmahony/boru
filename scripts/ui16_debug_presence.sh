#!/usr/bin/env bash
# Manual debug harness: launch boru with the figure-4 fixture, then
# set_peer_presence AFTER a delay, reporting the MCP action result.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
BIN=$ROOT/target/debug/boru
FIXTURE=$ROOT/scripts/figure4_fixture.py
MCP=$ROOT/scripts/ui_mcp.py
REMOTE_PK=28d7ee8656$(printf 'ab%.0s' {1..27})

display=${1:-240}
port=$((18800 + display))
data=$(mktemp -d /tmp/boru-ui16-dbg.XXXXXX)
python3 "$FIXTURE" inject "$data" >/dev/null

Xvfb ":$display" -screen 0 "1280x800x24" -nolisten tcp >/tmp/dbg-xvfb.log 2>&1 &
XVFB=$!
sleep 0.8
kill -0 "$XVFB" 2>/dev/null || { echo "Xvfb failed"; tail -5 /tmp/dbg-xvfb.log; exit 1; }

DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "dbg" \
  --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$port" open >/tmp/dbg-app.log 2>&1 &
APP=$!
echo "app pid $APP"

for _ in $(seq 1 100); do
  DISPLAY=":$display" python3 "$MCP" "$port" boru_ping '{}' >/dev/null 2>&1 && break
  sleep 0.25
done
echo "mcp ready"

DISPLAY=":$display" python3 "$MCP" "$port" boru_gui_open_conversation \
  "{\"conversation_id\":\"$REMOTE_PK\"}" | head -c 300; echo
echo "waiting 8s for friends/conversation load..."
sleep 8
echo "--- set_peer_presence online:true ---"
RESP=$(DISPLAY=":$display" python3 "$MCP" "$port" boru_gui_set_peer_presence \
  "{\"peer_id\":\"$REMOTE_PK\",\"online\":true}" 2>&1)
echo "$RESP"
ACTION_ID=$(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('result',{}).get('action_id',''))")
echo "action_id=$ACTION_ID"
sleep 3
echo "--- action status ---"
DISPLAY=":$display" python3 "$MCP" "$port" boru_gui_get_action_status "{\"action_id\":\"$ACTION_ID\"}" 2>&1 | head -c 500; echo
WIN=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' | head -1)
DISPLAY=":$display" import -window "$WIN" /tmp/dbg-capture.png
echo "captured /tmp/dbg-capture.png"

kill "$APP" "$XVFB" 2>/dev/null
wait "$APP" 2>/dev/null
wait "$XVFB" 2>/dev/null
python3 "$FIXTURE" cleanup "$data" >/dev/null 2>&1
