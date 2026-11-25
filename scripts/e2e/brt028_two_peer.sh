#!/usr/bin/env bash
set -euo pipefail

RUN_ID="$(date +%s)-$$-${RANDOM}"
ROOT="${TMPDIR:-/tmp}/brt028-${RUN_ID}"
TAURI_BIN="${TAURI_BIN:-desktop/src-tauri/target/debug/boru-desktop}"
PEER_BIN="${PEER_BIN:-target/debug/boru}"
STARTUP_TIMEOUT="${STARTUP_TIMEOUT:-60}"
ARTIFACT_DIR="${ARTIFACT_DIR:-}"
TAURI_PID=""
PEER_PID=""
KEEP_ARTIFACTS=false

if ! [[ "$STARTUP_TIMEOUT" =~ ^[1-9][0-9]*$ ]]; then
  printf 'STARTUP_TIMEOUT must be a positive integer\n' >&2
  exit 2
fi

mkdir -p "$ROOT/tauri/data" "$ROOT/tauri/config" "$ROOT/peer/data"
TAURI_LOG="$ROOT/tauri.log"
PEER_LOG="$ROOT/peer.log"

if [[ -n "$ARTIFACT_DIR" ]]; then
  mkdir -p "$ARTIFACT_DIR"
  ARTIFACT_DIR="$(cd "$ARTIFACT_DIR" && pwd)"
  KEEP_ARTIFACTS=true
fi

free_port() {
  python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'
}
TAURI_PORT="$(free_port)"
PEER_PORT="$(free_port)"
while [[ "$PEER_PORT" == "$TAURI_PORT" ]]; do PEER_PORT="$(free_port)"; done

finish() {
  local rc=$?
  trap - EXIT
  for pid in "$TAURI_PID" "$PEER_PID"; do
    [[ -n "$pid" ]] || continue
    kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
  done
  for pid in "$TAURI_PID" "$PEER_PID"; do
    [[ -n "$pid" ]] || continue
    for _ in {1..30}; do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.1
    done
    kill -KILL -- "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  if [[ "$KEEP_ARTIFACTS" == true ]]; then
    cp -f "$TAURI_LOG" "$PEER_LOG" "$ARTIFACT_DIR/" 2>/dev/null || true
  fi
  rm -rf -- "$ROOT"
  exit "$rc"
}
trap finish EXIT

fail() {
  printf 'BRT-028 two-peer launcher failed: %s\n' "$1" >&2
  for log in "$TAURI_LOG" "$PEER_LOG"; do
    if [[ -f "$log" ]]; then
      printf '\n--- %s ---\n' "$log" >&2
      tail -n 80 "$log" >&2 || true
    fi
  done
  exit 1
}

# setsid gives each child a process group so its descendants are cleaned up too.
if [[ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]] && command -v xvfb-run >/dev/null 2>&1; then
  setsid xvfb-run -a -s "-screen 0 1280x1024x24" env XDG_DATA_HOME="$ROOT/tauri/data" XDG_CONFIG_HOME="$ROOT/tauri/config" \
    "$TAURI_BIN" >"$TAURI_LOG" 2>&1 &
  TAURI_PID=$!
  setsid xvfb-run -a -s "-screen 0 1280x1024x24" "$PEER_BIN" --data-dir "$ROOT/peer/data" --mcp --mcp-bind "127.0.0.1:$PEER_PORT" \
    --enable-gui-test-actions --no-relay --no-dht --keep-running >"$PEER_LOG" 2>&1 &
  PEER_PID=$!
else
  setsid env XDG_DATA_HOME="$ROOT/tauri/data" XDG_CONFIG_HOME="$ROOT/tauri/config" \
    "$TAURI_BIN" >"$TAURI_LOG" 2>&1 &
  TAURI_PID=$!
  setsid "$PEER_BIN" --data-dir "$ROOT/peer/data" --mcp --mcp-bind "127.0.0.1:$PEER_PORT" \
    --enable-gui-test-actions --no-relay --no-dht --keep-running >"$PEER_LOG" 2>&1 &
  PEER_PID=$!
fi

wait_ready() {
  local name="$1" pid="$2" profile="$3" log="$4" marker="$5" deadline=$((SECONDS + STARTUP_TIMEOUT))
  while (( SECONDS < deadline )); do
    kill -0 "$pid" 2>/dev/null || fail "$name process exited before becoming ready"
    if [[ -d "$profile" ]] && { [[ "$marker" == FILE:* && -f "${marker#FILE:}" ]] || grep -Eiq "$marker" "$log" 2>/dev/null; }; then
      return 0
    fi
    sleep 0.2
  done
  fail "$name did not become ready within ${STARTUP_TIMEOUT}s"
}

wait_ready tauri "$TAURI_PID" "$ROOT/tauri/data/computer.iroh.boru.desktop" "$TAURI_LOG" "FILE:$ROOT/tauri/data/computer.iroh.boru.desktop/boru.db"
# The Iced peer writes tracing output to <data-dir>/logs/boru.log, not stdout:
# its stdout/stderr stay empty on a healthy start. Grep the tracing file.
wait_ready peer "$PEER_PID" "$ROOT/peer/data" "$ROOT/peer/data/logs/boru.log" 'MCP diagnostic server listening on'

python3 - "$RUN_ID" "$TAURI_PID" "$TAURI_PORT" "$ROOT/tauri/data" "$TAURI_LOG" "$PEER_PID" "$PEER_PORT" "$ROOT/peer/data" "$PEER_LOG" "${ARTIFACT_DIR:-$ROOT}" <<'PY'
import json, sys
run_id, tauri_pid, tauri_port, xdg_data, tauri_log, peer_pid, peer_port, data_dir, peer_log, artifacts = sys.argv[1:]
print(json.dumps({
    "run_id": run_id,
    "tauri": {"pid": int(tauri_pid), "port": int(tauri_port), "xdg_data_home": xdg_data, "log": tauri_log},
    "peer": {"pid": int(peer_pid), "data_dir": data_dir, "port": int(peer_port), "log": peer_log},
    "artifact_dir": artifacts,
}, separators=(",", ":")))
PY
