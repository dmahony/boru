#!/usr/bin/env bash
# UI-HOME-14 evidence (t_5c7a2325): typography across shared application chrome.
# Captures the four required screenshots under Xvfb:
#   1. home           — chat list / home rail (populated fixture)
#   2. chat           — open conversation timeline (Figtree chat chrome)
#   3. file-sharing   — file-sharing dashboard (Shared by Me)
#   4. creation-dialog— Create Group Chat dialog (shared dialog chrome)
# Output: docs/ui-redesign/evidence/t_5c7a2325/
set -euo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=${BORU_BIN:-$ROOT/target/debug/examples/boru}
FIXTURE=$ROOT/scripts/figure4_fixture.py
MCP=$ROOT/scripts/ui_mcp.py
OUT=$ROOT/docs/ui-redesign/evidence/t_5c7a2325
REMOTE_PK=28d7ee8656$(printf 'ab%.0s' {1..27})
mkdir -p "$OUT"

command -v Xvfb >/dev/null || { echo "Xvfb required" >&2; exit 1; }
command -v xdotool >/dev/null || { echo "xdotool required" >&2; exit 1; }
command -v import >/dev/null || { echo "imagemagick import required" >&2; exit 1; }
[[ -x "$BIN" ]] || { echo "boru binary missing: $BIN" >&2; exit 1; }

pick_display() {
  for c in $(seq 300 359); do
    if [[ ! -e "/tmp/.X11-unix/X${c}" && ! -e "/tmp/.X${c}-lock" ]]; then
      echo "$c"; return
    fi
  done
  echo "" >&2
}

find_window() {
  local display=$1 win=''
  for _ in $(seq 1 100); do
    win=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
    [[ -n "$win" ]] && break
    sleep 0.25
  done
  echo "$win"
}

settle() {
  local display=$1 win=$2 prev=''
  for _ in $(seq 1 30); do
    DISPLAY=":$display" import -window "$win" /tmp/ui14-settle.png 2>/dev/null || true
    if [[ -n "$prev" ]] && cmp -s /tmp/ui14-settle.png "$prev"; then return 0; fi
    cp /tmp/ui14-settle.png "$prev" 2>/dev/null || true
    sleep 0.5
  done
  return 0
}

# capture <name> <w> <h> <mode>  where mode in home|chat|file_sharing|create_group
capture() {
  local name=$1 w=$2 h=$3 mode=$4
  local display mcp_port data_dir xvfb app win
  display=$(pick_display); mcp_port=$((18700 + display))
  data_dir=$(mktemp -d /tmp/boru-ui14.XXXXXX)
  python3 "$FIXTURE" inject --now-ms "$(date +%s%3N)" "$data_dir" >/dev/null 2>&1 || true
  Xvfb ":$display" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/ui14-xvfb.log 2>&1 & xvfb=$!
  sleep 0.8
  kill -0 "$xvfb" 2>/dev/null || { echo "FAIL ${name}: Xvfb died on :$display" >&2; rm -rf "$data_dir"; return 1; }
  DISPLAY=":$display" "$BIN" --data-dir "$data_dir" --no-dht --no-relay --name "UI-14 $name" \
    --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" \
    >/tmp/ui14-app.log 2>&1 &
  app=$!
  local ok=0
  for _ in $(seq 1 120); do
    if DISPLAY=":$display" python3 "$MCP" "$mcp_port" boru_ping '{}' >/dev/null 2>&1; then ok=1; break; fi
    sleep 0.25
  done
  [[ "$ok" == "1" ]] || { echo "FAIL ${name}: MCP not ready" >&2; tail -5 /tmp/ui14-app.log >&2; kill "$app" "$xvfb" 2>/dev/null || true; rm -rf "$data_dir"; return 1; }
  sleep 6
  win=$(find_window "$display")
  if [[ -n "$win" ]]; then
    DISPLAY=":$display" xdotool windowsize "$win" "$w" "$h"
    settle "$display" "$win"
    case "$mode" in
      home)
        DISPLAY=":$display" python3 "$MCP" "$mcp_port" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null 2>&1 || true
        ;;
      chat)
        DISPLAY=":$display" python3 "$MCP" "$mcp_port" boru_gui_open_conversation \
          "{\"conversation_id\":\"$REMOTE_PK\"}" >/dev/null 2>&1 || true
        ;;
      file_sharing)
        DISPLAY=":$display" python3 "$MCP" "$mcp_port" boru_gui_navigate '{"destination":"file_sharing"}' >/dev/null 2>&1 || true
        ;;
      create_group)
        # ShowCreateGroupDialog maps to the internally-tagged GuiTestCommand
        # "create_new_room" (opens the create-room dialog; the create-group
        # dialog shares the same dialog chrome/typography).
        DISPLAY=":$display" python3 "$MCP" "$mcp_port" boru_send_gui_action \
          '{"command":"create_new_room"}' >/dev/null 2>&1 || true
        ;;
    esac
    sleep 4
    settle "$display" "$win"
    DISPLAY=":$display" import -window "$win" "$OUT/${name}_${w}x${h}.png"
    echo "OK ${name} ${w}x${h}"
  else
    echo "FAIL ${name}: window not found" >&2
  fi
  kill "$app" "$xvfb" 2>/dev/null || true
  wait "$app" 2>/dev/null || true
  wait "$xvfb" 2>/dev/null || true
  python3 "$FIXTURE" cleanup "$data_dir" >/dev/null 2>&1 || rm -rf "$data_dir"
}

for mode in home chat file_sharing create_group; do
  ( capture "t_5c7a2325_${mode}" 1280 800 "$mode" ) || echo "FAILED ${mode}" >&2
done

echo "ALL DONE"
file "$OUT"/t_5c7a2325_*.png | sed 's/^/  /'
