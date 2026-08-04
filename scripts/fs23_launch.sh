#!/usr/bin/env bash
# FS-23 clean-run launcher: isolated Xvfb + private dbus + portal + two boru peers.
# Usage: fs23_launch.sh start|stop|status
set -uo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=${BORU_BIN:-$ROOT/target/debug/examples/boru}
BASE=/tmp/fs23-clean
DISPLAY_NUM=310
MCP_SENDER=19101
MCP_RECEIVER=19102
BUS_PATH=/tmp/fs23-bus
PID_DIR=/tmp/fs23-clean/pids
mkdir -p "$PID_DIR"

pick_display() {
  for c in $(seq 310 359); do
    if [[ ! -e "/tmp/.X11-unix/X${c}" && ! -e "/tmp/.X${c}-lock" ]]; then
      echo "$c"; return
    fi
  done
  echo ""
}

start_env() {
  local disp
  disp=$(pick_display)
  DISPLAY_NUM=$disp
  echo "using display :$disp"
  # Plain Xvfb without the GLX extension flags. Earlier runs added
  # `+extension GLX +render` which crashes the X server itself after ~4 min
  # (killing every X client with it). wgpu/iced renders fine via llvmpipe
  # software GL with LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe set on
  # the app (see launch_app).
  Xvfb ":$disp" -screen 0 2000x800x24 -nolisten tcp -noreset \
    >"$BASE/xvfb.log" 2>&1 &
  echo $! >"$PID_DIR/xvfb.pid"
  sleep 1
  kill -0 "$(cat "$PID_DIR/xvfb.pid")" 2>/dev/null || { echo "Xvfb failed"; exit 1; }

  rm -f "$BUS_PATH"
  dbus-daemon --session --address="unix:path=$BUS_PATH" --fork --print-address >"$BASE/dbus.addr" 2>&1
  export DBUS_SESSION_BUS_ADDRESS="unix:path=$BUS_PATH"
  export DISPLAY=":$disp"
  export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
  # Propagate DISPLAY into the bus activation environment so D-Bus-activated
  # services (xdg-desktop-portal-gtk → rfd native file picker) can init GTK.
  # Without this, StartServiceByName spawns portal-gtk with no DISPLAY and it
  # exits status 1, so the native OS picker never appears.
  if command -v dbus-update-activation-environment >/dev/null 2>&1; then
    dbus-update-activation-environment DISPLAY=":$disp" XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
      DBUS_SESSION_BUS_ADDRESS="unix:path=$BUS_PATH" 2>/dev/null || true
  fi

  DISPLAY=":$disp" DBUS_SESSION_BUS_ADDRESS="unix:path=$BUS_PATH" \
    /usr/libexec/xdg-desktop-portal >"$BASE/portal.log" 2>&1 &
  echo $! >"$PID_DIR/portal.pid"
  sleep 0.5
  DISPLAY=":$disp" DBUS_SESSION_BUS_ADDRESS="unix:path=$BUS_PATH" \
    /usr/libexec/xdg-desktop-portal-gtk >"$BASE/portal-gtk.log" 2>&1 &
  echo $! >"$PID_DIR/portal-gtk.pid"
  sleep 1
}

launch_app() { # $1=name $2=datadir $3=mcp_port $4=bind_port
  local name=$1 data=$2 port=$3 bind_port=$4
  DISPLAY=":$DISPLAY_NUM" DBUS_SESSION_BUS_ADDRESS="unix:path=$BUS_PATH" \
    LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe \
    "$BIN" --data-dir "$data" --no-dht --no-relay --name "$name" \
    --bind-port "$bind_port" \
    --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$port" \
    >"$data/app.log" 2>&1 &
  echo $! >"$PID_DIR/$name.pid"
}

wait_mcp() { # $1=port $2=name
  local port=$1 name=$2 ok=0
  for _ in $(seq 1 80); do
    if python3 "$ROOT/scripts/fs23_mcp.py" "$port" boru_ping '{}' >/dev/null 2>&1; then ok=1; break; fi
    sleep 0.25
  done
  if [[ "$ok" == "1" ]]; then echo "MCP ready: $name ($port)"; else echo "MCP TIMEOUT: $name ($port)"; fi
}

start() {
  start_env
  # Staggered launches: mDNS announcements race when both peers start within
  # the same second, and iroh's mDNS service does not replay cached
  # advertisements to subscribers created after the event. Starting the
  # sender first lets its announcement settle so the receiver (whose lobby
  # subscription registers before its endpoint binds) observes it.
  launch_app sender "$BASE/sender" $MCP_SENDER 41001
  wait_mcp $MCP_SENDER sender
  sleep 4
  launch_app receiver "$BASE/receiver" $MCP_RECEIVER 41002
  wait_mcp $MCP_RECEIVER receiver
  echo "DISPLAY_NUM=$DISPLAY_NUM MCP_SENDER=$MCP_SENDER MCP_RECEIVER=$MCP_RECEIVER"
  echo "$DISPLAY_NUM" >"$PID_DIR/display"
  echo "$MCP_SENDER" >"$PID_DIR/mcp_sender"
  echo "$MCP_RECEIVER" >"$PID_DIR/mcp_receiver"
}

stop() {
  for f in sender receiver; do
    if [[ -s "$PID_DIR/$f.pid" ]]; then kill "$(cat "$PID_DIR/$f.pid")" 2>/dev/null || true; fi
  done
  sleep 2
  for f in sender receiver; do
    if [[ -s "$PID_DIR/$f.pid" ]]; then kill -9 "$(cat "$PID_DIR/$f.pid")" 2>/dev/null || true; fi
  done
  for f in portal portal-gtk xvfb; do
    if [[ -s "$PID_DIR/$f.pid" ]]; then kill "$(cat "$PID_DIR/$f.pid")" 2>/dev/null || true; fi
  done
  rm -f "$BUS_PATH"
  echo "stopped"
}

status() {
  for f in sender receiver portal portal-gtk xvfb; do
    if [[ -s "$PID_DIR/$f.pid" ]]; then
      local pid
      pid=$(cat "$PID_DIR/$f.pid")
      if kill -0 "$pid" 2>/dev/null; then echo "$f: RUNNING ($pid)"; else echo "$f: DEAD ($pid)"; fi
    else
      echo "$f: no pidfile"
    fi
  done
}

case "${1:-}" in
  start) start ;;
  stop) stop ;;
  status) status ;;
  *) echo "usage: $0 start|stop|status" >&2; exit 2 ;;
esac
