#!/usr/bin/env bash
# UI-21 final evidence (t_8960f71c): final visual regression and product review.
#
# Captures the real running Boru GUI under Xvfb into
# docs/ui-redesign/evidence/final/:
#   1. The screenshot matrix — home + chat at all four required viewports
#      (1024x720, 1280x800, 1440x900, 1920x1080) using the Figure-4 fixture.
#   2. The key-state matrix (1280x800): ready, connecting, offline/degraded,
#      empty/populated lists, selected chat, online/offline peer, empty/long
#      chat, sending/read/failed message, disabled composer.
#   3. Side-by-side 1280x800 comparisons against the target Figures 3 and 4.
#
# All states come from deterministic QA fixtures (scripts/figure4_fixture.py,
# scripts/ui14_states_evidence.sh spec, scripts/ui16_fixture.py) written into
# isolated temp data dirs — never production data. The binary is the running
# application; point BORU_BIN at a freshly built target/debug/examples/boru.
set -euo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=${BORU_BIN:-$ROOT/target/debug/examples/boru}
FIXTURE=$ROOT/scripts/figure4_fixture.py
UI16_FIXTURE=$ROOT/scripts/ui16_fixture.py
MCP=$ROOT/scripts/ui_mcp.py
OUT=$ROOT/docs/ui-redesign/evidence/final
STATES_SPEC=$ROOT/docs/ui-redesign/evidence/ui-14/ui14-states-spec.json
REMOTE_PK=28d7ee8656$(printf 'ab%.0s' {1..27})
mkdir -p "$OUT"

command -v Xvfb >/dev/null || { echo "Xvfb required" >&2; exit 1; }
command -v xdotool >/dev/null || { echo "xdotool required" >&2; exit 1; }
command -v import >/dev/null || { echo "imagemagick import required" >&2; exit 1; }
[[ -x "$BIN" ]] || { echo "boru binary missing: $BIN" >&2; exit 1; }

pick_display() {
  for c in $(seq 300 359); do
    if [[ ! -e "/tmp/.X11-unix/X${c}" && ! -e "/tmp/.X${c}-lock" ]]; then
      echo "$c"
      return
    fi
  done
  echo "" >&2
}

launch() { # $1=display $2=mcp_port $3=name $4=data_dir $5=open_conv $6=presence $7=home_mode
  local display=$1 port=$2 name=$3 data=$4 open_conv=${5:-false} presence=${6:-true} home_mode=${7:-false}
  if [[ "$home_mode" == "true" ]]; then
    DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "$name" \
      --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$port" \
      >/tmp/ui21-app.log 2>&1 &
  else
    DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "$name" \
      --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$port" open \
      >/tmp/ui21-app.log 2>&1 &
  fi
  APP_PID=$!
  local ok=0
  for _ in $(seq 1 120); do
    if DISPLAY=":$display" python3 "$MCP" "$port" boru_ping '{}' >/dev/null 2>&1; then ok=1; break; fi
    sleep 0.25
  done
  [[ "$ok" == "1" ]] || { echo "MCP not ready for $name" >&2; tail -5 /tmp/ui21-app.log >&2; return 1; }
  if [[ "$open_conv" == "true" ]]; then
    DISPLAY=":$display" python3 "$MCP" "$port" boru_gui_open_conversation \
      "{\"conversation_id\":\"$REMOTE_PK\"}" >/dev/null 2>&1 || true
  fi
  sleep 8
  if [[ "$presence" == "true" ]]; then
    local attempt=0 applied=0 resp aid state
    while [[ $attempt -lt 4 && "$applied" == "0" ]]; do
      attempt=$((attempt + 1))
      resp=$(DISPLAY=":$display" python3 "$MCP" "$port" boru_gui_set_peer_presence \
        "{\"peer_id\":\"$REMOTE_PK\",\"online\":true}" 2>/dev/null) || { sleep 2; continue; }
      aid=$(echo "$resp" | python3 -c "import sys,json;print(json.load(sys.stdin)['result']['action_id'])" 2>/dev/null || true)
      if [[ -n "$aid" ]]; then
        for _ in $(seq 1 12); do
          sleep 0.5
          state=$(DISPLAY=":$display" python3 "$MCP" "$port" boru_gui_get_action_status \
            "{\"action_id\":\"$aid\"}" 2>/dev/null | python3 -c "import sys,json;print(json.load(sys.stdin)['result']['status']['state'])" 2>/dev/null || true)
          if [[ "$state" == "completed" ]]; then applied=1; break; fi
          if [[ "$state" == "rejected" || "$state" == "error" ]]; then break; fi
        done
      fi
      sleep 1
    done
    [[ "$applied" == "1" ]] || echo "WARN: set_peer_presence not confirmed for $name" >&2
  fi
}

set_peer_offline() { # $1=display $2=mcp_port — report peer offline (footer "Not connected")
  local display=$1 port=$2
  DISPLAY=":$display" python3 "$MCP" "$port" boru_gui_set_peer_presence \
    "{\"peer_id\":\"$REMOTE_PK\",\"online\":false}" >/dev/null 2>&1 || true
  sleep 2
}

settle() { # $1=display $2=window_id
  local display=$1 win=$2 prev=''
  for _ in $(seq 1 30); do
    DISPLAY=":$display" import -window "$win" /tmp/ui21-settle.png 2>/dev/null || true
    if [[ -n "$prev" ]] && cmp -s /tmp/ui21-settle.png "$prev"; then return 0; fi
    cp /tmp/ui21-settle.png "$prev" 2>/dev/null || true
    sleep 0.5
  done
  return 0
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

navigate_home() { # $1=display $2=mcp_port
  DISPLAY=":$1" python3 "$MCP" "$2" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null 2>&1 || true
  sleep 1.5
}

# capture <name> <w> <h> <fixture_cmd...> [--no-presence] [--open] [--home] [--offline]
capture() {
  local name=$1 w=$2 h=$3
  shift 3
  local presence=true open_conv=false home_mode=false offline=false
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --no-presence) presence=false; shift ;;
      --open) open_conv=true; shift ;;
      --home) home_mode=true; shift ;;
      --offline) offline=true; shift ;;
      *) break ;;
    esac
  done
  local display mcp_port data_dir xvfb app win
  display=$(pick_display); mcp_port=$((18700 + display))
  data_dir=$(mktemp -d /tmp/boru-ui21.XXXXXX)
  "$@" "$data_dir" >/dev/null
  Xvfb ":$display" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/ui21-xvfb.log 2>&1 & xvfb=$!
  sleep 0.8
  kill -0 "$xvfb" 2>/dev/null || { echo "FAIL ${name}: Xvfb died on :$display" >&2; rm -rf "$data_dir"; return 1; }
  APP_PID=""
  launch "$display" "$mcp_port" "UI-21 $name" "$data_dir" "$open_conv" "$presence" "$home_mode" \
    || { kill "$xvfb" 2>/dev/null || true; rm -rf "$data_dir"; return 1; }
  app=$APP_PID
  win=$(find_window "$display")
  if [[ -n "$win" ]]; then
    DISPLAY=":$display" xdotool windowsize "$win" "$w" "$h"
    settle "$display" "$win"
    if [[ "$home_mode" == "true" ]]; then
      navigate_home "$display" "$mcp_port"
      settle "$display" "$win"
    fi
    if [[ "$offline" == "true" ]]; then
      set_peer_offline "$display" "$mcp_port"
      settle "$display" "$win"
    fi
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

now_ms=$(date +%s%3N)

# ── 1) Screenshot matrix: home + chat at the four required viewports ──────
for spec in '1024 720' '1280 800' '1440 900' '1920 1080'; do
  read -r w h <<<"$spec"
  ( capture final_home "$w" "$h" --home python3 "$FIXTURE" inject --now-ms "$now_ms" ) \
    || echo "FAILED final_home ${w}x${h}" >&2
  ( capture final_chat "$w" "$h" --open python3 "$FIXTURE" inject --now-ms "$now_ms" ) \
    || echo "FAILED final_chat ${w}x${h}" >&2
done

# ── 2) State matrix (1280x800) ────────────────────────────────────────────
# Ready (populated, online peer): the figure-4 home with presence online.
( capture state_ready 1280 800 --home python3 "$FIXTURE" inject --now-ms "$now_ms" ) \
  || echo "FAILED state_ready" >&2
# Connecting (no fixture; bootstrap state).
( capture state_connecting 1280 800 --home --no-presence python3 "$FIXTURE" inject --now-ms "$now_ms" ) \
  || echo "FAILED state_connecting" >&2
# Offline/degraded footer: figure-4 chat, peer reported offline.
( capture state_offline 1280 800 --open --offline python3 "$FIXTURE" inject --now-ms "$now_ms" ) \
  || echo "FAILED state_offline" >&2
# Empty lists (fresh data dir, no fixture).
( capture state_empty_lists 1280 800 --home --no-presence python3 "$FIXTURE" inject --now-ms "$now_ms" --force ) \
  || echo "FAILED state_empty_lists" >&2
# Populated lists: the figure-4 fixture sidebar (chats + friend).
( capture state_populated 1280 800 --home python3 "$FIXTURE" inject --now-ms "$now_ms" ) \
  || echo "FAILED state_populated" >&2
# Selected chat: conversation open (selected in sidebar + active timeline).
( capture state_selected_chat 1280 800 --open python3 "$FIXTURE" inject --now-ms "$now_ms" ) \
  || echo "FAILED state_selected_chat" >&2
# Online peer (header presence label + dot).
( capture state_peer_online 1280 800 --open python3 "$FIXTURE" inject --now-ms "$now_ms" ) \
  || echo "FAILED state_peer_online" >&2
# Offline peer (header presence shows offline).
( capture state_peer_offline 1280 800 --open --offline python3 "$FIXTURE" inject --now-ms "$now_ms" ) \
  || echo "FAILED state_peer_offline" >&2
# Empty chat timeline.
( capture state_empty_chat 1280 800 --open python3 "$UI16_FIXTURE" empty --now-ms "$now_ms" ) \
  || echo "FAILED state_empty_chat" >&2
# Long chat (400 messages).
( capture state_long_chat 1280 800 --open python3 "$UI16_FIXTURE" long --count 400 --now-ms "$now_ms" ) \
  || echo "FAILED state_long_chat" >&2
# Sending/read/failed ladder (ui14 states spec).
( capture state_message_ladder 1280 800 --open python3 "$FIXTURE" inject --spec "$STATES_SPEC" --now-ms "$now_ms" ) \
  || echo "FAILED state_message_ladder" >&2
# Disabled composer (empty composer, muted disabled send circle).
( capture state_composer_disabled 1280 800 --open python3 "$UI16_FIXTURE" empty --now-ms "$now_ms" ) \
  || echo "FAILED state_composer_disabled" >&2

echo "ALL DONE"
file "$OUT"/final_*.png "$OUT"/state_*.png | sed 's/^/  /'
