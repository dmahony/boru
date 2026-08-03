#!/usr/bin/env bash
# UI-18 evidence (t_f75e5521): responsive resizing and high-DPI validation.
#
# Captures the real GUI under Xvfb for:
#   1. Home + chat at the four required viewports (1024x720, 1280x800,
#      1440x900, 1920x1080) — the screenshot matrix.
#   2. Long-value stress (long friend labels, long group name, long unbroken
#      messages, long system events) at 1024x720 and 1280x800.
#   3. High-DPI at 125%, 150% and 200% (winit X11 scale-factor override), a
#      1280x800 logical window at each factor.
#   4. Continuous drag-resize sweep across intermediate sizes plus a
#      return-to-reference capture to prove layout stability (no jitter /
#      breakpoint oscillation) while the app stays responsive.
#
# All states come from deterministic QA fixtures (scripts/figure4_fixture.py
# and scripts/ui18_fixture.py) written into isolated temp data dirs — never
# production data.
set -euo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=${BORU_BIN:-$ROOT/target/debug/examples/boru}
FIXTURE=$ROOT/scripts/figure4_fixture.py
UI18_FIXTURE=$ROOT/scripts/ui18_fixture.py
MCP=$ROOT/scripts/ui_mcp.py
OUT=$ROOT/docs/ui-redesign/evidence/ui-18
REMOTE_PK=28d7ee8656$(printf 'ab%.0s' {1..27})
mkdir -p "$OUT"

command -v Xvfb >/dev/null || { echo "Xvfb required" >&2; exit 1; }
command -v xdotool >/dev/null || { echo "xdotool required" >&2; exit 1; }
command -v import >/dev/null || { echo "imagemagick import required" >&2; exit 1; }
command -v compare >/dev/null || { echo "imagemagick compare required" >&2; exit 1; }
[[ -x "$BIN" ]] || { echo "boru binary missing: $BIN" >&2; exit 1; }

pick_display() {
  for c in $(seq 200 259); do
    if [[ ! -e "/tmp/.X11-unix/X${c}" && ! -e "/tmp/.X${c}-lock" ]]; then
      echo "$c"
      return
    fi
  done
  echo "" >&2
}

launch() { # $1=display $2=mcp_port $3=name $4=data_dir $5=open_conv(true|false) $6=presence(true|false) $7=home_mode(true|false)
  local display=$1 port=$2 name=$3 data=$4 open_conv=${5:-false} presence=${6:-true} home_mode=${7:-false}
  # home_mode omits the `open` subcommand so `args.command.is_none()` makes
  # return_to_chat_list_after_open=true: after the lobby subscription the UI
  # deterministically returns to the ChatList (home) instead of racing into
  # the lobby chat. This is how UI-11/UI-15 captured the home matrix.
  if [[ "$home_mode" == "true" ]]; then
    DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "$name" \
      --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$port" \
      >/tmp/ui18-app.log 2>&1 &
  else
    DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "$name" \
      --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$port" open \
      >/tmp/ui18-app.log 2>&1 &
  fi
  APP_PID=$!
  local ok=0
  for _ in $(seq 1 120); do
    if DISPLAY=":$display" python3 "$MCP" "$port" boru_ping '{}' >/dev/null 2>&1; then ok=1; break; fi
    sleep 0.25
  done
  [[ "$ok" == "1" ]] || { echo "MCP not ready for $name" >&2; tail -5 /tmp/ui18-app.log >&2; return 1; }
  if [[ "$open_conv" == "true" ]]; then
    DISPLAY=":$display" python3 "$MCP" "$port" boru_gui_open_conversation \
      "{\"conversation_id\":\"$REMOTE_PK\"}" >/dev/null
  fi
  # Friends/conversation stores load asynchronously; wait before queuing the
  # presence action so SetPeerPresence validation (peer must be a known
  # friend) succeeds instead of being rejected.
  sleep 8
  if [[ "$presence" == "true" ]]; then
    local attempt=0 applied=0
    while [[ $attempt -lt 4 && "$applied" == "0" ]]; do
      attempt=$((attempt + 1))
      local resp aid state
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

settle() { # $1=display $2=window_id
  local display=$1 win=$2 prev=''
  for _ in $(seq 1 30); do
    DISPLAY=":$display" import -window "$win" /tmp/ui18-settle.png 2>/dev/null || true
    if [[ -n "$prev" ]] && cmp -s /tmp/ui18-settle.png "$prev"; then return 0; fi
    cp /tmp/ui18-settle.png "$prev" 2>/dev/null || true
    sleep 0.5
  done
  return 0
}

find_window() { # $1=display — echoes window id (empty on failure)
  local display=$1 win=''
  for _ in $(seq 1 100); do
    win=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
    [[ -n "$win" ]] && break
    sleep 0.25
  done
  echo "$win"
}

navigate_home() { # $1=display $2=mcp_port — force the ChatList (home) screen.
  # Deterministic: after the lobby subscription the app may be on the lobby
  # chat; GoToChatList via the same MCP action the visible UI uses guarantees
  # the home screen no matter the lobby race timing.
  DISPLAY=":$1" python3 "$MCP" "$2" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null 2>&1 || true
  sleep 1.5
}

# capture_scenario <name> <width> <height> <fixture_cmd...> [--no-presence] [--open] [--home]
# Launches on its own Xvfb display sized w x h, sizes the window to w x h,
# settles and captures. Extra flags are consumed in order.
capture_scenario() {
  local name=$1 w=$2 h=$3
  shift 3
  local presence=true open_conv=false home_mode=false
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --no-presence) presence=false; shift ;;
      --open) open_conv=true; shift ;;
      --home) home_mode=true; shift ;;
      *) break ;;
    esac
  done
  local display mcp_port data_dir
  display=$(pick_display); mcp_port=$((18700 + display))
  data_dir=$(mktemp -d /tmp/boru-ui18.XXXXXX)
  "$@" "$data_dir" >/dev/null
  local xvfb app win
  Xvfb ":$display" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/ui18-xvfb.log 2>&1 & xvfb=$!
  sleep 0.8
  kill -0 "$xvfb" 2>/dev/null || { echo "FAIL ${name}: Xvfb died on :$display" >&2; rm -rf "$data_dir"; return 1; }
  APP_PID=""
  launch "$display" "$mcp_port" "UI-18 $name" "$data_dir" "$open_conv" "$presence" "$home_mode" \
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

# capture_dpi <factor> <name> [home] — 1280x800 logical window at the given scale.
# Xvfb runs at physical size (logical * factor); the window is sized to the
# physical size; winit derives the logical size from the scale factor.
capture_dpi() {
  local factor=$1 name=$2 home_mode=${3:-false}
  local pw ph
  pw=$(python3 -c "print(int(round(1280*$factor)))")
  ph=$(python3 -c "print(int(round(800*$factor)))")
  local display mcp_port data_dir
  display=$(pick_display); mcp_port=$((18700 + display))
  data_dir=$(mktemp -d /tmp/boru-ui18-dpi.XXXXXX)
  python3 "$FIXTURE" inject "$data_dir" --now-ms 1752000000000 >/dev/null
  local xvfb app win
  Xvfb ":$display" -screen 0 "${pw}x${ph}x24" -nolisten tcp >/tmp/ui18-xvfb-dpi.log 2>&1 & xvfb=$!
  sleep 0.8
  kill -0 "$xvfb" 2>/dev/null || { echo "FAIL dpi${factor}: Xvfb died" >&2; rm -rf "$data_dir"; return 1; }
  APP_PID=""
  WINIT_X11_SCALE_FACTOR="$factor" \
  launch "$display" "$mcp_port" "UI-18 dpi $factor" "$data_dir" false true "$home_mode" \
    || { kill "$xvfb" 2>/dev/null || true; rm -rf "$data_dir"; return 1; }
  app=$APP_PID
  win=$(find_window "$display")
  if [[ -n "$win" ]]; then
    DISPLAY=":$display" xdotool windowsize "$win" "$pw" "$ph"
    settle "$display" "$win"
    if [[ "$home_mode" == "true" ]]; then
      navigate_home "$display" "$mcp_port"
      settle "$display" "$win"
    fi
    DISPLAY=":$display" import -window "$win" "$OUT/${name}_dpi${factor}.png"
    local geom
    geom=$(DISPLAY=":$display" xdotool getwindowgeometry "$win" 2>/dev/null | tr '\n' ' ')
    echo "OK ${name} dpi ${factor} (physical ${pw}x${ph}; $geom)"
  else
    echo "FAIL dpi${factor}: window not found" >&2
  fi
  kill "$app" "$xvfb" 2>/dev/null || true
  wait "$app" 2>/dev/null || true
  wait "$xvfb" 2>/dev/null || true
  python3 "$FIXTURE" cleanup "$data_dir" >/dev/null 2>&1 || rm -rf "$data_dir"
}

now_ms=$(date +%s%3N)

# ── 1) Screenshot matrix: home + chat at the four required viewports ─────
for spec in '1024 720' '1280 800' '1440 900' '1920 1080'; do
  read -r w h <<<"$spec"
  ( capture_scenario ui18_home "$w" "$h" --home python3 "$FIXTURE" inject --now-ms "$now_ms" ) \
    || echo "FAILED home ${w}x${h}" >&2
  ( capture_scenario ui18_chat "$w" "$h" --open python3 "$FIXTURE" inject --now-ms "$now_ms" ) \
    || echo "FAILED chat ${w}x${h}" >&2
done

# ── 2) Long-value stress at the smallest and reference viewports ─────────
for spec in '1024 720' '1280 800'; do
  read -r w h <<<"$spec"
  ( capture_scenario ui18_stress_home "$w" "$h" --home python3 "$UI18_FIXTURE" stress --now-ms "$now_ms" ) \
    || echo "FAILED stress home ${w}x${h}" >&2
  ( capture_scenario ui18_stress_chat "$w" "$h" --open python3 "$UI18_FIXTURE" stress --now-ms "$now_ms" ) \
    || echo "FAILED stress chat ${w}x${h}" >&2
done

# ── 3) High-DPI: 125 / 150 / 200 percent, 1280x800 logical ───────────────
for factor in 1.25 1.5 2.0; do
  ( capture_dpi "$factor" ui18_home home ) || echo "FAILED dpi home ${factor}" >&2
  ( capture_dpi "$factor" ui18_chat ) || echo "FAILED dpi chat ${factor}" >&2
done

# ── 4) Continuous drag-resize sweep + stability re-check ─────────────────
(
display=$(pick_display); mcp_port=$((18700 + display))
data_dir=$(mktemp -d /tmp/boru-ui18-sweep.XXXXXX)
python3 "$FIXTURE" inject "$data_dir" --now-ms "$now_ms" >/dev/null
Xvfb ":$display" -screen 0 "1920x1080x24" -nolisten tcp >/tmp/ui18-xvfb-sweep.log 2>&1 & xvfb=$!
sleep 0.8
kill -0 "$xvfb" 2>/dev/null || { echo "FAIL sweep: Xvfb died" >&2; rm -rf "$data_dir"; exit 1; }
APP_PID=""
launch "$display" "$mcp_port" "UI-18 sweep" "$data_dir" false true true || { kill "$xvfb" 2>/dev/null || true; rm -rf "$data_dir"; exit 1; }
app=$APP_PID
win=$(find_window "$display")
if [[ -n "$win" ]]; then
  # Deterministically start from the home (ChatList) screen.
  navigate_home "$display" "$mcp_port"
  settle "$display" "$win"
  # Start at the smallest supported size, then walk up through intermediate
  # sizes (including the 1040 px quick-action column boundary), then back to
  # the reference size for a stability re-check.
  i=0
  for spec in '1024 720' '1080 720' '1152 768' '1280 800' '1366 768' '1440 900' '1600 900' '1920 1080' '1280 800'; do
    read -r w h <<<"$spec"
    i=$((i+1))
    DISPLAY=":$display" xdotool windowsize "$win" "$w" "$h"
    sleep 0.6
    settle "$display" "$win"
    DISPLAY=":$display" import -window "$win" "$OUT/ui18_sweep_${i}_${w}x${h}.png"
    echo "OK sweep ${w}x${h} (frame $i)"
  done
  # App must still be alive and responsive after the sweep.
  if DISPLAY=":$display" python3 "$MCP" "$mcp_port" boru_ping '{}' >/dev/null 2>&1; then
    echo "OK sweep: MCP responsive after resize sweep"
  else
    echo "FAIL sweep: MCP unresponsive after resize sweep" >&2
  fi
else
  echo "FAIL sweep: window not found" >&2
fi
kill "$app" "$xvfb" 2>/dev/null || true
wait "$app" 2>/dev/null || true
wait "$xvfb" 2>/dev/null || true
python3 "$FIXTURE" cleanup "$data_dir" >/dev/null 2>&1 || rm -rf "$data_dir"
) || echo "FAILED resize sweep" >&2

echo "ALL DONE"
file "$OUT"/ui18_*.png | sed 's/^/  /'
