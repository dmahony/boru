#!/bin/bash
# Start boru headless with xvfb-run, MCP enabled.
# Usage: ./start_boru_headless.sh <remote-host> <name> <mcp-port> [data-dir]
set -e

HOST=$1
NAME=$2
MCP_PORT=$3
DATA_DIR=${4:-/tmp/boru_data_$NAME}

ssh -o ConnectTimeout=10 -o BatchMode=yes "$HOST" bash -s -- <<ENDSSH
  # Kill existing
  pkill -f "boru.*$NAME" 2>/dev/null || true
  sleep 1
  rm -rf "$DATA_DIR" 2>/dev/null || true

  # Start with xvfb-run
  nohup xvfb-run -a -e /tmp/xvfb_${NAME}.log \
    /home/dan/boru \
    --relay boru.chat:8443 \
    --mcp --enable-gui-test-actions \
    --mcp-bind "127.0.0.1:$MCP_PORT" \
    --name "$NAME" \
    --data-dir "$DATA_DIR" \
    open \
    > /tmp/boru_${NAME}.log 2>&1 &

  BORU_PID=\$!
  echo "Started $NAME PID=\$BORU_PID"

  # Wait for MCP port
  for i in \$(seq 1 60); do
    if ss -tlnp 2>/dev/null | grep -q "$MCP_PORT"; then
      echo "MCP ready after \${i}s"
      break
    fi
    sleep 1
  done
  if ss -tlnp 2>/dev/null | grep -q "$MCP_PORT"; then
    echo "SUCCESS: $NAME MCP running on port $MCP_PORT"
  else
    echo "FAIL: $NAME MCP never started"
    cat "$DATA_DIR"/logs/boru.log 2>/dev/null | tail -20 || echo "No log"
    cat /tmp/boru_${NAME}.log 2>/dev/null | tail -5 || echo "No stdout"
  fi
ENDSSH
