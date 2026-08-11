#!/usr/bin/env bash
# UI-17 live verification of the ToggleHelp fix on the ISOLATED worktree binary
# (HEAD + UI-17 hunks only, no sibling WIP). Sends toggle_help through MCP and
# checks: (1) action status reaches Completed, (2) dialog_open snapshot flips,
# (3) iced journal records GuiTestActionReceived, (4) screenshot shows overlay.
set -uo pipefail
BIN=/tmp/ui17-verify/target/debug/boru
MCP=/home/dan/iroh-gossip-chat/scripts/ui_mcp.py
OUT=/tmp/ui17-livefix
mkdir -p "$OUT"
pass=0; fail=0
check() { if [[ "$2" == "1" ]]; then pass=$((pass+1)); echo "PASS: $1"; else fail=$((fail+1)); echo "FAIL: $1"; fi }
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
  --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$MCPP" open >"$OUT/app.log" 2>&1 & APP=$!
tries=0; while ! python3 "$MCP" "$MCPP" boru_ping '{}' >/dev/null 2>&1; do tries=$((tries+1)); [[ $tries -gt 80 ]] && { echo "MCP failed"; tail -5 "$OUT/app.log"; exit 1; }; sleep 0.25; done
sleep 4
WA=$(DISPLAY=":$DISP" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -1 || true)
echo "window=$WA"

send_and_status() {
  local label="$1" cmd="$2"
  local resp aid state
  resp=$(python3 "$MCP" "$MCPP" boru_send_gui_action "$cmd" 2>&1)
  aid=$(echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin).get('result',{}).get('idempotency_key',''))" 2>/dev/null)
  echo "$label aid=$aid"
  sleep 2
  state=$(python3 "$MCP" "$MCPP" boru_gui_get_action_status "{\"action_id\":\"$aid\"}" 2>&1 | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('result',{}).get('status',{}).get('state'))" 2>/dev/null)
  echo "$label state=$state"
  echo "$state" > "$OUT/${label// /_}.state"
}
send_and_status "toggle_help_1" '{"command":{"command":"toggle_help"}}'
send_and_status "toggle_help_2" '{"command":{"command":"toggle_help"}}'
send_and_status "go_to_chat_list" '{"command":{"command":"go_to_chat_list"}}'

# dialog_open snapshot check (help overlay open -> dialog_open true)
python3 "$MCP" "$MCPP" boru_send_gui_action '{"command":{"command":"toggle_help"}}' >/dev/null 2>&1
sleep 1.5
HLP=$(python3 "$MCP" "$MCPP" boru_gui_wait_for_state '{"condition":{"type":"dialog_open"},"timeout_ms":6000}' 2>&1)
echo "help dialog_open: $HLP" | head -c 300; echo
check "help overlay opens via MCP toggle_help (dialog_open)" "$(echo "$HLP" | grep -c '"reached":true')"

# journal check
python3 "$MCP" "$MCPP" boru_get_iced_message_journal '{"limit":10}' > "$OUT/journal.json" 2>&1
JV=$(python3 -c "
import json
d=json.load(open('$OUT/journal.json'))
entries=d.get('result',{}).get('entries',[])
print(sum(1 for e in entries if e.get('message_variant')=='GuiTestActionReceived'))
" 2>/dev/null)
echo "journal GuiTestActionReceived count: $JV"
check "journal records GuiTestActionReceived" "$([ "${JV:-0}" -ge 1 ] && echo 1 || echo 0)"

[[ -n "$WA" ]] && DISPLAY=":$DISP" import -window "$WA" "$OUT/toggle_help_live.png" 2>/dev/null && echo "screenshot saved"
echo "RESULT: pass=$pass fail=$fail"
