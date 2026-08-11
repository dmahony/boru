#!/usr/bin/env bash
# UI-16 evidence (t_dfbde136): chat footer, full composition and scroll polish.
#
# Captures the real GUI under Xvfb for: the Figure-4 short conversation at all
# four required viewports, the empty conversation, a one-message conversation,
# a 400-message long history, the offline footer state, and a live
# resize-while-sending sequence. All states come from deterministic QA fixtures
# (scripts/figure4_fixture.py / scripts/ui16_fixture.py) written into isolated
# temp data dirs — never production data.
set -euo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=$ROOT/target/debug/boru
FIXTURE=$ROOT/scripts/figure4_fixture.py
UI16_FIXTURE=$ROOT/scripts/ui16_fixture.py
MCP=$ROOT/scripts/ui_mcp.py
OUT=$ROOT/docs/ui-redesign/evidence/ui-16
REMOTE_PK=28d7ee8656$(printf 'ab%.0s' {1..27})
mkdir -p "$OUT"

command -v Xvfb >/dev/null || { echo "Xvfb required" >&2; exit 1; }
command -v xdotool >/dev/null || { echo "xdotool required" >&2; exit 1; }
command -v import >/dev/null || { echo "imagemagick import required" >&2; exit 1; }
[[ -x "$BIN" ]] || { echo "boru binary missing: $BIN" >&2; exit 1; }

pick_display() {
  for c in $(seq 260 299); do
    if [[ ! -e "/tmp/.X11-unix/X${c}" && ! -e "/tmp/.X${c}-lock" ]]; then
      echo "$c"
      return
    fi
  done
  echo "" >&2
}

launch() { # $1=display $2=mcp_port $3=name $4=data_dir $5=online(true|false)
  local display=$1 port=$2 name=$3 data=$4 online=${5:-true}
  DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "$name" \
    --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$port" open \
    >/tmp/ui16-app.log 2>&1 &
  APP_PID=$!
  local ok=0
  for _ in $(seq 1 100); do
    if DISPLAY=":$display" python3 "$MCP" "$port" boru_ping '{}' >/dev/null 2>&1; then ok=1; break; fi
    sleep 0.25
  done
  [[ "$ok" == "1" ]] || { echo "MCP not ready for $name" >&2; tail -5 /tmp/ui16-app.log >&2; return 1; }
  DISPLAY=":$display" python3 "$MCP" "$port" boru_gui_open_conversation \
    "{\"conversation_id\":\"$REMOTE_PK\"}" >/dev/null
  # Friends/conversation stores load asynchronously; wait before queuing the
  # presence action so the SetPeerPresence validation (peer must be a known
  # friend) succeeds instead of being rejected.
  sleep 8
  local attempt=0 applied=0
  while [[ $attempt -lt 4 && "$applied" == "0" ]]; do
    attempt=$((attempt + 1))
    local resp aid state
    resp=$(DISPLAY=":$display" python3 "$MCP" "$port" boru_gui_set_peer_presence \
      "{\"peer_id\":\"$REMOTE_PK\",\"online\":$online}" 2>/dev/null) || { sleep 2; continue; }
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
  [[ "$applied" == "1" ]] || echo "WARN: set_peer_presence not confirmed for $name (online=$online)" >&2
}

settle() { # $1=display $2=window_id
  local display=$1 win=$2 prev=''
  for _ in $(seq 1 30); do
    DISPLAY=":$display" import -window "$win" /tmp/ui16-settle.png 2>/dev/null || true
    if [[ -n "$prev" ]] && cmp -s /tmp/ui16-settle.png "$prev"; then return 0; fi
    cp /tmp/ui16-settle.png "$prev" 2>/dev/null || true
    sleep 0.5
  done
  return 0
}

capture_scenario() { # $1=scenario_name $2=width $3=height [$4=offline] inject_cmd...
  local name=$1 w=$2 h=$3
  shift 3
  local online=true
  if [[ "$1" == "offline" ]]; then online=false; shift; fi
  local display mcp_port data_dir
  display=$(pick_display); mcp_port=$((18700 + display))
  data_dir=$(mktemp -d /tmp/boru-ui16.XXXXXX)
  # shellcheck disable=SC2086
  "$@" "$data_dir" >/dev/null
  local xvfb app
  Xvfb ":$display" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/ui16-xvfb.log 2>&1 & xvfb=$!
  sleep 0.8
  kill -0 "$xvfb" 2>/dev/null || { echo "FAIL ${name}: Xvfb died on :$display" >&2; rm -rf "$data_dir"; return 1; }
  APP_PID=""
  launch "$display" "$mcp_port" "UI-16 $name" "$data_dir" "$online" || { kill "$xvfb" 2>/dev/null || true; rm -rf "$data_dir"; return 1; }
  app=$APP_PID
  local win=''
  for _ in $(seq 1 80); do
    win=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
    [[ -n "$win" ]] && break
    sleep 0.25
  done
  if [[ -n "$win" ]]; then
    DISPLAY=":$display" xdotool windowsize "$win" "$w" "$h"
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

now_ms=$(date +%s%3N)

# 1) Figure-4 short conversation at all four required viewports.
for spec in '1280 800' '1024 720' '1440 900' '1920 1080'; do
  read -r w h <<<"$spec"
  ( capture_scenario t_dfbde136_figure4 "$w" "$h" python3 "$FIXTURE" inject --now-ms "$now_ms" ) \
    || echo "FAILED figure4 ${w}x${h}" >&2
done

# 2) Empty conversation.
( capture_scenario t_dfbde136_empty 1280 800 python3 "$UI16_FIXTURE" empty --now-ms "$now_ms" ) \
  || echo "FAILED empty 1280x800" >&2
( capture_scenario t_dfbde136_empty 1024 720 python3 "$UI16_FIXTURE" empty --now-ms "$now_ms" ) \
  || echo "FAILED empty 1024x720" >&2

# 3) One-message conversation.
( capture_scenario t_dfbde136_one 1280 800 python3 "$UI16_FIXTURE" one --now-ms "$now_ms" ) \
  || echo "FAILED one 1280x800" >&2

# 4) Long history (400 messages) — bottom (latest pinned above composer).
( capture_scenario t_dfbde136_long 1280 800 python3 "$UI16_FIXTURE" long --count 400 --now-ms "$now_ms" ) \
  || echo "FAILED long 1280x800" >&2

# 5) Offline footer state: figure-4 timeline, peer reported offline -> "Not connected".
( capture_scenario t_dfbde136_offline 1280 800 offline python3 "$FIXTURE" inject --now-ms "$now_ms" ) \
  || echo "FAILED offline 1280x800" >&2
# 6) Live resize while receiving messages: one instance, four viewports,
#    a fresh message submitted at each size.
(
display=$(pick_display); mcp_port=$((18700 + display))
data_dir=$(mktemp -d /tmp/boru-ui16-live.XXXXXX)
python3 "$FIXTURE" inject "$data_dir" --now-ms "$now_ms" >/dev/null
Xvfb ":$display" -screen 0 "1920x1080x24" -nolisten tcp >/tmp/ui16-xvfb-live.log 2>&1 & xvfb=$!
sleep 0.8
kill -0 "$xvfb" 2>/dev/null || { echo "FAIL live: Xvfb died" >&2; rm -rf "$data_dir"; exit 1; }
APP_PID=""
launch "$display" "$mcp_port" "UI-16 live resize" "$data_dir" || { kill "$xvfb" 2>/dev/null || true; rm -rf "$data_dir"; exit 1; }
app=$APP_PID
win=''
for _ in $(seq 1 80); do
  win=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
  [[ -n "$win" ]] && break
  sleep 0.25
done
if [[ -n "$win" ]]; then
  i=0
  for spec in '1024 720' '1280 800' '1440 900' '1920 1080'; do
    read -r w h <<<"$spec"
    i=$((i+1))
    DISPLAY=":$display" xdotool windowsize "$win" "$w" "$h"
    sleep 1
    DISPLAY=":$display" python3 "$MCP" "$mcp_port" boru_gui_set_composer \
      "{\"text\":\"Live resize message $i — latest stays visible above the composer.\"}" >/dev/null
    sleep 0.5
    DISPLAY=":$display" python3 "$MCP" "$mcp_port" boru_gui_submit_composer '{}' >/dev/null
    settle "$display" "$win"
    DISPLAY=":$display" import -window "$win" "$OUT/t_dfbde136_live_resize_${w}x${h}.png"
    echo "OK live_resize ${w}x${h} (message $i sent)"
  done
else
  echo "FAIL live resize: window not found" >&2
fi
kill "$app" "$xvfb" 2>/dev/null || true
wait "$app" 2>/dev/null || true
wait "$xvfb" 2>/dev/null || true
python3 "$FIXTURE" cleanup "$data_dir" >/dev/null 2>&1 || rm -rf "$data_dir"
) || echo "FAILED live resize" >&2

echo "ALL DONE"
file "$OUT"/t_dfbde136_*.png
