#!/usr/bin/env bash
# UI-17 help overlay VISUAL verification: open a seeded direct conversation,
# then toggle_help via MCP, then screenshot the chat panel overlay.
set -uo pipefail
BIN=/tmp/ui17-verify/target/debug/examples/boru
MCP=/home/dan/iroh-gossip-chat/scripts/ui_mcp.py
OUT=/tmp/ui17-livefix
PK_B=7d59c5623dd40a74aa4d5a32ac645d3b3f95daeae4c22be25476dd6a486f7382
cleanup() { kill "${APP:-}" "${XV:-}" 2>/dev/null || true; wait "${APP:-}" "${XV:-}" 2>/dev/null || true; }
trap cleanup EXIT
pkill -f 'boru --data-dir /tmp/ui17-livefix' 2>/dev/null || true
[[ -e "/tmp/.X11-unix/X272" ]] && kill "$(pgrep -f 'Xvfb :272 ' | head -1)" 2>/dev/null || true
sleep 1
rm -rf /tmp/ui17-livefix-data
python3 /home/dan/iroh-gossip-chat/scripts/seed_two_instances.py /tmp/ui17-livefix-data >/dev/null 2>&1 || true
DISP=272; MCPP=$((18700 + DISP))
Xvfb ":$DISP" -screen 0 "1280x800x24" -nolisten tcp >"$OUT/xvfb.log" 2>&1 & XV=$!
sleep 1
DISPLAY=":$DISP" "$BIN" --data-dir /tmp/ui17-livefix-data --bind-port 43121 --no-dht --no-relay --name "APeer" \
  --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$MCPP" open >"$OUT/app2.log" 2>&1 & APP=$!
tries=0; while ! python3 "$MCP" "$MCPP" boru_ping '{}' >/dev/null 2>&1; do tries=$((tries+1)); [[ $tries -gt 80 ]] && { echo "MCP failed"; exit 1; }; sleep 0.25; done
sleep 4
python3 "$MCP" "$MCPP" boru_gui_open_conversation "{\"conversation_id\":\"$PK_B\"}" >/dev/null 2>&1
sleep 3
WA=$(DISPLAY=":$DISP" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -1 || true)
echo "window=$WA"
python3 "$MCP" "$MCPP" boru_send_gui_action '{"command":{"command":"toggle_help"}}' >/dev/null 2>&1
sleep 1.5
HLP=$(python3 "$MCP" "$MCPP" boru_gui_wait_for_state '{"condition":{"type":"dialog_open"},"timeout_ms":6000}' 2>&1)
echo "dialog_open reached: $(echo "$HLP" | grep -c '"reached":true')"
[[ -n "$WA" ]] && DISPLAY=":$DISP" import -window "$WA" "$OUT/help_overlay_chat.png" 2>/dev/null && echo "saved $OUT/help_overlay_chat.png"
